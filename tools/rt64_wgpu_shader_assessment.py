#!/usr/bin/env python3
"""Assess the accepted RT64 reference SPIR-V corpus through strict wgpu 30."""

from __future__ import annotations

import argparse
import copy
import json
import os
import stat
import struct
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

try:
    import resource
except ImportError:  # pragma: no cover - diagnostic census is not certified on Windows.
    resource = None

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rt64_reference_shader_artifacts as reference
import rt64_shader_artifacts as base


ROOT = Path(__file__).resolve().parents[1]
TOOL_PATH = Path(__file__).resolve()
POLICY_PATH = ROOT / "docs/rt64-wgpu-shader-assessment-schema.json"
RECEIPT_NAME = "assessment-receipt.json"
EMPTY_SHA256 = base.digest_bytes(b"")
KNOWN_STDERR = b"fn64-wgpu-shader-validator: wgpu 30 SPIR-V parse failed: unsupported capability ShaderNonUniform\n"
SCALAR_LAYOUT_STDERR = b"fn64-wgpu-shader-validator: wgpu 30 naga validation failed: Global variable [0] 'instanceRDPParams' is invalid\n"
SAMPLED_BUFFER_STDERR = b"fn64-wgpu-shader-validator: wgpu 30 SPIR-V parse failed: unsupported capability SampledBuffer\n"
FRAGMENT_INTERFACE_STDERR = b"fn64-wgpu-shader-validator: wgpu 30 naga validation failed: Entry point PSMain at Fragment is invalid\n"
RUNTIME_NOT_READY_EXIT = 78
PROFILE_EXTENTS = (4, 8, 16, 20, 24, 32, 40, 56)
PROFILE_NAMES = ("baseline", *(f"immediates-{extent}" for extent in PROFILE_EXTENTS))
IMMEDIATE_WITNESS_SCHEMA = "fn64.spirv-immediate-profile-witness.v1"
VALIDATOR_RESULT_SCHEMA = "fn64.wgpu-shader-validation.v2"
DIAGNOSTIC_CENSUS_SCHEMA = "fn64.rt64-wgpu-shader-diagnostic-census.v1"


def validator_profile(name: str) -> dict:
    base.require(name in PROFILE_NAMES, f"unknown closed validator profile: {name}")
    extent = 0 if name == "baseline" else int(name.removeprefix("immediates-"))
    return {
        "name": name,
        "required_features": [] if extent == 0 else ["IMMEDIATES"],
        "required_limits": {"max_immediate_size": extent},
    }


VALIDATOR_PROFILES = [validator_profile(name) for name in PROFILE_NAMES]


def validator_arguments(profile_name: str, stage: str, entry: str) -> list[str]:
    validator_profile(profile_name)
    return [
        "--profile", profile_name,
        "--shader", "<private-staged-spv>",
        "--stage", stage,
        "--entry", entry,
    ]


def validator_success_record(profile_name: str, stage: str, entry: str, module_bytes: int) -> dict:
    return {
        "schema": VALIDATOR_RESULT_SCHEMA,
        "status": "passed",
        "wgpu_major": 30,
        "profile": validator_profile(profile_name),
        "stage": stage,
        "entry": entry,
        "module_bytes": module_bytes,
    }


def validator_success_bytes(profile_name: str, stage: str, entry: str, module_bytes: int) -> bytes:
    return (json.dumps(
        validator_success_record(profile_name, stage, entry, module_bytes),
        separators=(",", ":"),
    ) + "\n").encode()


def safe_relative(value: object, label: str) -> PurePosixPath:
    base.require(isinstance(value, str), f"{label} is not text")
    path = PurePosixPath(value)
    base.require(path.parts and not path.is_absolute() and ".." not in path.parts, f"unsafe {label}")
    base.require(path.as_posix() == value, f"non-canonical {label}")
    return path


def require_sha(value: object, label: str) -> str:
    base.require(isinstance(value, str) and base.SHA256_RE.fullmatch(value) is not None, f"{label} is not a SHA-256 digest")
    return value


def load_exact_pretty_json_bytes(value: bytes, label: str) -> dict:
    try:
        result = json.loads(value)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise base.ArtifactError(f"{label} is malformed: {error}") from error
    base.require(isinstance(result, dict), f"{label} must contain a JSON object")
    base.require(base.pretty_json(result) == value, f"{label} is not exact pretty JSON")
    return result


def load_policy() -> dict:
    policy = base.load_json(POLICY_PATH)
    base.require_keys(policy, {
        "schema", "direct_consumers", "receipt_schema", "receipt_path", "producer", "reference_corpus",
        "wgpu_validator", "profile_derivation", "outcomes", "diagnostic_census",
        "runtime_readiness", "limits", "claim_boundary",
    }, "wgpu assessment policy")
    base.require(policy["schema"] == "fn64.rt64-wgpu-shader-assessment-policy.v3", "unsupported wgpu assessment policy")
    base.require(policy["direct_consumers"] == [
        "tools/rt64_wgpu_shader_assessment.py",
        "tools/test_rt64_wgpu_shader_assessment.py",
    ], "wgpu assessment policy consumer denominator changed")
    base.require(policy["receipt_schema"] == "fn64.rt64-wgpu-shader-assessment.v3", "unsupported wgpu assessment receipt schema")
    base.require(policy["receipt_path"] == RECEIPT_NAME, "wgpu assessment receipt path changed")
    base.require_keys(policy["producer"], {"path", "sha256"}, "wgpu assessment producer")
    base.require(policy["producer"]["path"] == "tools/rt64_wgpu_shader_assessment.py", "wgpu assessment producer path changed")
    base.require(require_sha(policy["producer"]["sha256"], "wgpu assessment producer digest") == base.digest_file(TOOL_PATH), "wgpu assessment producer digest changed")
    base.require_keys(policy["reference_corpus"], {
        "receipt_schema", "receipt_sha256", "receipt_file_sha256", "artifact_set_sha256",
        "denominator_sha256", "source_snapshot_set_sha256", "orchestration_producer_sha256",
        "artifact_producer_sha256", "reference_policy_sha256", "artifact_policy_sha256",
        "dxc_build_receipt_sha256", "dxc_compiler_sha256", "spirv_val_build_receipt_sha256",
        "spirv_grammar_sha256", "entry_order_sha256", "row_count", "file_count",
    }, "reference corpus policy")
    for key in (
        "receipt_sha256", "receipt_file_sha256", "artifact_set_sha256", "denominator_sha256",
        "source_snapshot_set_sha256", "orchestration_producer_sha256", "artifact_producer_sha256",
        "reference_policy_sha256", "artifact_policy_sha256", "dxc_build_receipt_sha256",
        "dxc_compiler_sha256", "spirv_val_build_receipt_sha256", "spirv_grammar_sha256",
        "entry_order_sha256",
    ):
        require_sha(policy["reference_corpus"][key], f"reference corpus {key}")
    base.require(policy["reference_corpus"]["receipt_schema"] == "fn64.rt64-reference-shader-receipt.v2", "reference receipt schema changed")
    base.require(policy["reference_corpus"]["row_count"] == 56, "reference row denominator changed")
    base.require(policy["reference_corpus"]["file_count"] == 225, "reference file denominator changed")
    validator = policy["wgpu_validator"]
    base.require_keys(validator, {
        "build_receipt_schema", "build_receipt_sha256", "binary_sha256", "source_set_sha256",
        "cargo_lock_sha256", "dependency_set_sha256", "artifact_identity_status",
        "identity", "arguments", "controlled_environment",
    }, "wgpu validator policy")
    validator_hash_keys = ("build_receipt_sha256", "binary_sha256", "source_set_sha256", "cargo_lock_sha256", "dependency_set_sha256")
    if validator["artifact_identity_status"] == "pending-m2.4-v2-integration":
        base.require(all(validator[key] is None for key in validator_hash_keys), "pending M2.4 v2 validator identity contains guessed hashes")
    else:
        base.require(validator["artifact_identity_status"] == "frozen", "wgpu validator artifact identity status changed")
        for key in validator_hash_keys:
            require_sha(validator[key], f"wgpu validator {key}")
    base.require(validator["build_receipt_schema"] == "fn64.wgpu-shader-validator-build.v2", "wgpu validator build schema changed")
    base.require_keys(validator["identity"], {"schema", "wgpu_major", "wgpu_version", "naga_version", "backend", "validation", "profiles"}, "wgpu validator identity policy")
    base.require(validator["identity"] == {
        "schema": "fn64.wgpu-shader-validator.v2", "wgpu_major": 30,
        "wgpu_version": "30.0.0", "naga_version": "30.0.0", "backend": "noop",
        "validation": "wgpu-30-closed-profile-naga-validation-plus-checked-api",
        "profiles": VALIDATOR_PROFILES,
    }, "wgpu validator identity denominator changed")
    base.require(validator["arguments"] == ["--profile", "<derived-profile>", "--shader", "<private-staged-spv>", "--stage", "<stage>", "--entry", "<entry>"], "wgpu validator argv changed")
    base.require(validator["controlled_environment"] == {
        "HOME": "<private-staging-root>", "LANG": "C", "LC_ALL": "C", "PATH": "/usr/bin:/bin",
    }, "wgpu validator environment changed")
    derivation = policy["profile_derivation"]
    base.require_keys(derivation, {
        "witness_schema", "push_constant_storage_class", "block_decoration",
        "offset_decoration", "scalar_alignment", "vector_alignments",
        "closed_profiles", "rounded_spans", "expected_corpus_profile_counts",
    }, "validator profile derivation")
    base.require(derivation == {
        "witness_schema": IMMEDIATE_WITNESS_SCHEMA,
        "push_constant_storage_class": 9,
        "block_decoration": 2,
        "offset_decoration": 35,
        "scalar_alignment": 4,
        "vector_alignments": {"2": 8, "3": 16, "4": 16},
        "closed_profiles": list(PROFILE_NAMES),
        "rounded_spans": list(PROFILE_EXTENTS),
        "expected_corpus_profile_counts": {
            "baseline": 13, "immediates-4": 6, "immediates-8": 3,
            "immediates-16": 9, "immediates-20": 1, "immediates-24": 5,
            "immediates-32": 13, "immediates-40": 5, "immediates-56": 1,
        },
    }, "validator profile derivation changed")
    outcomes = policy["outcomes"]
    base.require_keys(outcomes, {
        "order", "ingestible", "blocked_known_shader_nonuniform", "blocked_known_scalar_layout",
        "blocked_known_sampled_buffer", "blocked_known_fragment_direct_blend_src_index_output",
    }, "wgpu outcome policy")
    base.require(outcomes["order"] == ["ingestible", "blocked-known"], "wgpu outcome order changed")
    base.require_keys(outcomes["ingestible"], {"exit_code", "stdout_schema", "stdout_status", "stderr_sha256"}, "ingestible outcome")
    base.require(outcomes["ingestible"] == {
        "exit_code": 0, "stdout_schema": VALIDATOR_RESULT_SCHEMA,
        "stdout_status": "passed", "stderr_sha256": EMPTY_SHA256,
    }, "ingestible outcome changed")
    blocked = outcomes["blocked_known_shader_nonuniform"]
    base.require_keys(blocked, {
        "reason_code", "exit_code", "stdout_sha256", "stderr_sha256", "required_capability",
        "required_extension", "required_direct_decoration",
    }, "blocked-known outcome")
    base.require_keys(blocked["required_capability"], {"name", "value"}, "blocked-known capability")
    base.require(blocked == {
        "reason_code": "naga30-strict-spv-unsupported-capability-shader-nonuniform",
        "exit_code": 2, "stdout_sha256": EMPTY_SHA256,
        "stderr_sha256": base.digest_bytes(KNOWN_STDERR),
        "required_capability": {"name": "ShaderNonUniform", "value": 5301},
        "required_extension": "SPV_EXT_descriptor_indexing",
        "required_direct_decoration": "NonUniform",
    }, "blocked-known outcome changed")
    scalar = outcomes["blocked_known_scalar_layout"]
    base.require_keys(scalar, {"reason_code", "exit_code", "stdout_sha256", "stderr", "stderr_sha256", "witness"}, "blocked-known scalar-layout outcome")
    base.require_keys(scalar["witness"], {
        "schema", "variable_name", "storage_class", "buffer_block", "descriptor_set", "binding",
        "container_name", "runtime_array_stride", "struct_name", "member_index", "member_name",
        "member_type", "member_offset", "required_alignment", "offset_aligned",
    }, "scalar-layout witness policy")
    base.require(scalar == {
        "reason_code": "naga30-standard-storage-layout-rejects-dxc-scalar-layout-rdpparams-keyscale",
        "exit_code": 2, "stdout_sha256": EMPTY_SHA256,
        "stderr": SCALAR_LAYOUT_STDERR.decode(), "stderr_sha256": base.digest_bytes(SCALAR_LAYOUT_STDERR),
        "witness": {
            "schema": "fn64.spirv-scalar-layout-witness.v1", "variable_name": "instanceRDPParams",
            "storage_class": "Uniform", "buffer_block": True, "descriptor_set": 0, "binding": 2,
            "container_name": "type.StructuredBuffer.RDPParams", "runtime_array_stride": 128,
            "struct_name": "RDPParams", "member_index": 7, "member_name": "keyScale",
            "member_type": "float3", "member_offset": 92, "required_alignment": 16,
            "offset_aligned": False,
        },
    }, "blocked-known scalar-layout outcome changed")
    sampled_buffer = outcomes["blocked_known_sampled_buffer"]
    base.require_keys(sampled_buffer, {"reason_code", "exit_code", "stdout_sha256", "stderr_sha256", "required_capability"}, "blocked-known sampled-buffer outcome")
    base.require_keys(sampled_buffer["required_capability"], {"name", "value"}, "blocked-known sampled-buffer capability")
    base.require(sampled_buffer == {
        "reason_code": "naga30-strict-spv-unsupported-capability-sampled-buffer",
        "exit_code": 2, "stdout_sha256": EMPTY_SHA256,
        "stderr_sha256": base.digest_bytes(SAMPLED_BUFFER_STDERR),
        "required_capability": {"name": "SampledBuffer", "value": 46},
    }, "blocked-known sampled-buffer outcome changed")
    fragment_interface = outcomes["blocked_known_fragment_direct_blend_src_index_output"]
    base.require_keys(fragment_interface, {"reason_code", "exit_code", "stdout_sha256", "stderr_sha256", "witness"}, "blocked-known fragment-interface outcome")
    base.require_keys(fragment_interface["witness"], {
        "schema", "stage", "entry", "variable_name", "storage_class", "type",
        "direct_interface_member", "location", "index",
    }, "blocked-known fragment-interface witness policy")
    base.require(fragment_interface == {
        "reason_code": "naga30-fragment-direct-blend-src-index-output",
        "exit_code": 2, "stdout_sha256": EMPTY_SHA256,
        "stderr_sha256": base.digest_bytes(FRAGMENT_INTERFACE_STDERR),
        "witness": {
            "schema": "fn64.spirv-fragment-blend-src-index-output-witness.v1",
            "stage": "fragment", "entry": "PSMain", "variable_name": "out.var.SV_TARGET0",
            "storage_class": "Output", "type": "float4", "direct_interface_member": True,
            "location": 0, "index": 0,
        },
    }, "blocked-known fragment-interface outcome changed")
    census = policy["diagnostic_census"]
    base.require_keys(census, {
        "schema", "authority", "row_count", "maximum_stream_bytes",
        "maximum_row_output_bytes", "maximum_total_output_bytes",
        "maximum_text_bytes", "maximum_census_bytes", "claim_boundary",
    }, "diagnostic census policy")
    base.require(census == {
        "schema": DIAGNOSTIC_CENSUS_SCHEMA,
        "authority": "non-authoritative-diagnostic-only",
        "row_count": 56,
        "maximum_stream_bytes": 4096,
        "maximum_row_output_bytes": 8192,
        "maximum_total_output_bytes": 458752,
        "maximum_text_bytes": 1024,
        "maximum_census_bytes": 4194304,
        "claim_boundary": "diagnostic-census-only-not-assessment-receipt-ingestion-runtime-readiness-or-parity",
    }, "diagnostic census policy changed")
    readiness = policy["runtime_readiness"]
    base.require_keys(readiness, {"runtime_ready", "reason_order"}, "runtime readiness policy")
    base.require(readiness == {
        "runtime_ready": False,
        "reason_order": [
            "blocked-known-ingestion-row", "native-adapter-contract-not-recorded",
            "native-shader-module-not-executed", "pipeline-and-semantic-evidence-not-recorded",
        ],
    }, "runtime readiness policy changed")
    base.require_keys(policy["limits"], {"maximum_receipt_bytes", "maximum_artifact_bytes", "maximum_process_output_bytes", "validator_timeout_seconds"}, "wgpu assessment limits")
    base.require(policy["limits"] == {"maximum_receipt_bytes": 16777216, "maximum_artifact_bytes": 67108864, "maximum_process_output_bytes": 65536, "validator_timeout_seconds": 30}, "wgpu assessment limits changed")
    base.require(policy["claim_boundary"] == "wgpu30-typed-ingestion-assessment-only-not-adapter-device-pipeline-runtime-parity-or-performance", "wgpu assessment claim boundary changed")
    return policy


def verify_reference_inputs(args: argparse.Namespace, policy: dict) -> dict:
    # M2.5a is accepted historical evidence. Requiring its mutable source/build
    # checkout here would silently replace that accepted identity with current
    # producer bytes. Re-authenticate the exact retained receipt and every file
    # it binds instead.
    corpus_dir = Path(args.reference_artifact_dir).resolve(strict=True)
    receipt_path = corpus_dir / reference.RECEIPT_PATH
    receipt_bytes, info = base.stable_regular_bytes(receipt_path, policy["limits"]["maximum_receipt_bytes"], "accepted reference receipt")
    base.require(info.st_nlink == 1, "accepted reference receipt has another hardlink")
    base.require(base.digest_bytes(receipt_bytes) == policy["reference_corpus"]["receipt_file_sha256"], "accepted reference receipt file identity changed")
    receipt = load_exact_pretty_json_bytes(receipt_bytes, "accepted reference receipt")
    expected = policy["reference_corpus"]
    base.validate_receipt_hash(receipt)
    base.require(receipt.get("schema") == expected["receipt_schema"], "accepted reference receipt schema changed")
    base.require(receipt.get("receipt_sha256") == expected["receipt_sha256"], "accepted reference receipt identity changed")
    base.require(receipt.get("artifact_set_sha256") == expected["artifact_set_sha256"], "accepted artifact set changed")
    base.require(receipt.get("denominator_sha256") == expected["denominator_sha256"], "accepted denominator changed")
    base.require(receipt.get("source_snapshot", {}).get("source_set_sha256") == expected["source_snapshot_set_sha256"], "accepted source snapshot changed")
    bindings = {
        "orchestration_producer_sha256": receipt.get("orchestration_producer_sha256"),
        "artifact_producer_sha256": receipt.get("artifact_producer_sha256"),
        "reference_policy_sha256": receipt.get("reference_policy_sha256"),
        "artifact_policy_sha256": receipt.get("artifact_policy_sha256"),
        "dxc_build_receipt_sha256": receipt.get("dxc_build_receipt_sha256"),
        "dxc_compiler_sha256": receipt.get("dxc_compiler_sha256"),
        "spirv_val_build_receipt_sha256": receipt.get("spirv_val_build_receipt_sha256"),
        "spirv_grammar_sha256": receipt.get("spirv_grammar", {}).get("sha256"),
    }
    base.require(bindings == {key: expected[key] for key in bindings}, "accepted reference toolchain binding changed")
    base.require(isinstance(receipt.get("entries"), list) and len(receipt["entries"]) == expected["row_count"], "accepted reference row denominator changed")
    base.require(base.digest_bytes(base.canonical_json([row.get("id") for row in receipt["entries"]])) == expected["entry_order_sha256"], "accepted reference row order changed")
    expected_files = {reference.RECEIPT_PATH}
    seen_objects = {(info.st_dev, info.st_ino)}
    artifact_set = []
    artifact_fields = (
        ("dependency_output_artifact", "dependency_output_sha256", "dependency_output_bytes"),
        ("dependency_manifest_artifact", "dependency_manifest_sha256", "dependency_manifest_bytes"),
        ("preprocessed_artifact", "preprocessed_sha256", "preprocessed_bytes"),
        ("spirv_artifact", "spirv_sha256", "spirv_bytes"),
    )
    seen_ids = set()
    for index, row in enumerate(receipt["entries"]):
        base.require(isinstance(row, dict), f"accepted reference row {index} is not an object")
        row_id = row.get("id")
        base.require(isinstance(row_id, str) and row_id and row_id.replace("-", "").isalnum() and row_id not in seen_ids, "accepted reference row id repeated or unsafe")
        seen_ids.add(row_id)
        safe_relative(row.get("source"), f"accepted reference source {row_id}")
        base.require(row.get("stage") in {"vertex", "fragment", "compute"}, f"accepted reference row stage changed: {row_id}")
        entry = row.get("entry")
        base.require(
            isinstance(entry, str) and entry.isascii() and entry
            and (entry[0].isalpha() or entry[0] == "_")
            and all(character.isalnum() or character == "_" for character in entry[1:]),
            f"accepted reference row entry changed: {row_id}",
        )
        validate_inventory_record(row.get("semantic_inventory"), f"accepted reference inventory {row_id}")
        for path_key, digest_key, length_key in artifact_fields:
            relative = safe_relative(row.get(path_key), f"accepted reference {path_key} {row_id}")
            relative_text = relative.as_posix()
            base.require(relative_text not in expected_files, f"accepted reference artifact path repeated: {relative_text}")
            expected_files.add(relative_text)
            digest = require_sha(row.get(digest_key), f"accepted reference {digest_key} {row_id}")
            length = row.get(length_key)
            base.require(isinstance(length, int) and not isinstance(length, bool) and length > 0, f"accepted reference {length_key} is invalid: {row_id}")
            artifact_path = corpus_dir.joinpath(*relative.parts)
            parent_rows = base.contained_parent_identities(corpus_dir, artifact_path, f"accepted reference artifact {relative_text}")
            artifact_bytes, artifact_info = base.stable_regular_bytes(
                artifact_path, policy["limits"]["maximum_artifact_bytes"], f"accepted reference artifact {relative_text}"
            )
            reference.verify_identity_rows(parent_rows, f"accepted reference artifact parents {relative_text}")
            identity = (artifact_info.st_dev, artifact_info.st_ino)
            base.require(artifact_info.st_nlink == 1 and identity not in seen_objects, f"accepted reference artifact is linked or reused: {relative_text}")
            seen_objects.add(identity)
            base.require(len(artifact_bytes) == length, f"accepted reference artifact length changed: {relative_text}")
            base.require(base.digest_bytes(artifact_bytes) == digest, f"accepted reference artifact digest changed: {relative_text}")
        artifact_set.append({"path": row["spirv_artifact"], "sha256": row["spirv_sha256"]})
    base.require(base.digest_bytes(base.canonical_json(artifact_set)) == expected["artifact_set_sha256"], "accepted reference artifact-set reconstruction changed")
    actual_files = set()
    for directory, names, files in os.walk(corpus_dir, followlinks=False):
        directory_path = Path(directory)
        for name in names:
            base.require(not (directory_path / name).is_symlink(), f"accepted reference corpus contains a directory symlink: {(directory_path / name).relative_to(corpus_dir)}")
        for name in files:
            actual_files.add((directory_path / name).relative_to(corpus_dir).as_posix())
    base.require(len(actual_files) == expected["file_count"] and actual_files == expected_files, f"accepted reference file denominator changed: {sorted(actual_files ^ expected_files)}")
    return receipt


def verify_validator_build(build_dir: Path, policy: dict) -> tuple[dict, Path, dict]:
    base.require(policy["wgpu_validator"]["artifact_identity_status"] == "frozen", "M2.4 validator v2 artifact identity is not integrated")
    receipt, binary, identity = base.validate_validator_build(build_dir)
    expected = policy["wgpu_validator"]
    base.require(receipt.get("schema") == expected["build_receipt_schema"], "wgpu validator build schema changed")
    base.require(receipt.get("receipt_sha256") == expected["build_receipt_sha256"], "wgpu validator build identity changed")
    base.require(receipt.get("binary_sha256") == expected["binary_sha256"], "wgpu validator binary identity changed")
    base.require(receipt.get("source_set_sha256") == expected["source_set_sha256"], "wgpu validator source identity changed")
    base.require(receipt.get("cargo_lock_sha256") == expected["cargo_lock_sha256"], "wgpu validator lock identity changed")
    base.require(receipt.get("dependency_set_sha256") == expected["dependency_set_sha256"], "wgpu validator dependency identity changed")
    base.require(identity.get("identity") == expected["identity"], "wgpu validator protocol identity changed")
    return receipt, binary, identity


@dataclass(frozen=True)
class QualifiedRoot:
    path: Path
    identity: tuple[int, int]
    parents: tuple


def qualify_root(path: Path, label: str) -> QualifiedRoot:
    resolved = path.resolve(strict=True)
    info = resolved.lstat()
    base.require(stat.S_ISDIR(info.st_mode) and not resolved.is_symlink(), f"{label} is not a regular directory")
    parents = tuple(base.contained_parent_identities(Path(resolved.anchor), resolved / ".fn64-boundary", label))
    return QualifiedRoot(resolved, (info.st_dev, info.st_ino), parents)


def verify_qualified_root(root: QualifiedRoot, label: str) -> None:
    reference.verify_identity_rows(root.parents, label)
    info = root.path.lstat()
    base.require((info.st_dev, info.st_ino) == root.identity and stat.S_ISDIR(info.st_mode) and not root.path.is_symlink(), f"{label} identity changed")


def qualify_private_root(path: Path, excluded: list[QualifiedRoot]) -> tuple:
    resolved = path.resolve(strict=True)
    base.require(resolved != ROOT and ROOT not in resolved.parents, "wgpu assessment staging overlaps fn64")
    rows = tuple(base.contained_parent_identities(Path(resolved.anchor), resolved / ".fn64-boundary", "wgpu assessment staging"))
    staging_objects = {identity[:2] for _, identity in rows}
    for item in excluded:
        verify_qualified_root(item, "wgpu assessment excluded root")
        base.require(item.identity not in staging_objects, "wgpu assessment staging contains an excluded directory identity")
        base.require(resolved != item.path and item.path not in resolved.parents and resolved not in item.path.parents, "wgpu assessment staging overlaps an excluded tree")
    path.chmod(0o700)
    base.require(stat.S_IMODE(path.stat().st_mode) == 0o700, "wgpu assessment staging root is not private")
    reference.verify_identity_rows(rows, "wgpu assessment staging")
    return rows


def controlled_environment(private_root: Path, policy: dict) -> dict[str, str]:
    configured = policy["wgpu_validator"]["controlled_environment"]
    return {"HOME": str(private_root), "LANG": configured["LANG"], "LC_ALL": configured["LC_ALL"], "PATH": configured["PATH"]}


def stage_validator(binary: Path, private_root: Path, policy: dict) -> Path:
    data, info = base.stable_regular_bytes(binary, policy["limits"]["maximum_artifact_bytes"], "qualified wgpu validator")
    base.require(info.st_nlink == 1, "qualified wgpu validator has another hardlink")
    base.require(base.digest_bytes(data) == policy["wgpu_validator"]["binary_sha256"], "qualified wgpu validator bytes changed")
    staged = private_root / "validator"
    base.write_new_private_file(staged, data)
    staged.chmod(0o500)
    base.require(base.digest_file(staged) == policy["wgpu_validator"]["binary_sha256"], "staged wgpu validator bytes changed")
    identity = base.validator_identity(staged)
    base.require(identity["identity"] == policy["wgpu_validator"]["identity"], "staged wgpu validator identity changed")
    return staged


def derive_validator_profile(artifact_bytes: bytes, policy: dict) -> dict:
    base.require(len(artifact_bytes) >= 20 and len(artifact_bytes) % 4 == 0, "profile SPIR-V extent is invalid")
    words = list(struct.unpack(f"<{len(artifact_bytes) // 4}I", artifact_bytes))
    base.require(words[0] == 0x07230203 and words[3] >= 1 and words[4] == 0, "profile SPIR-V header is invalid")
    bound = words[3]
    names: dict[int, str] = {}
    member_names: dict[tuple[int, int], str] = {}
    decorations: dict[tuple[int, int], tuple[int, ...]] = {}
    member_decorations: dict[tuple[int, int, int], tuple[int, ...]] = {}
    scalar_types: dict[int, tuple[str, int]] = {}
    vector_types: dict[int, tuple[int, int]] = {}
    struct_types: dict[int, tuple[int, ...]] = {}
    pointer_types: dict[int, tuple[int, int]] = {}
    variables: dict[int, tuple[int, int]] = {}
    unsupported_types: dict[int, str] = {}
    defined_results: set[int] = set()

    def valid_id(value: int, label: str) -> int:
        base.require(0 < value < bound, f"{label} is outside the SPIR-V id bound")
        return value

    def put_unique(table: dict, key: object, value: object, label: str) -> None:
        base.require(key not in table, f"duplicate {label}")
        table[key] = value

    def put_type(table: dict, result_id: int, value: object) -> None:
        base.require(result_id not in defined_results, "duplicate profile result id")
        defined_results.add(result_id)
        table[result_id] = value

    offset = 5
    while offset < len(words):
        first = words[offset]
        word_count = first >> 16
        opcode = first & 0xFFFF
        base.require(word_count > 0 and offset + word_count <= len(words), f"malformed profile SPIR-V instruction at word {offset}")
        operands = words[offset + 1 : offset + word_count]
        if opcode in {73, 74, 75}:
            raise base.ArtifactError(f"profile group decoration is not implemented at word {offset}")
        if opcode == 5:  # OpName
            base.require(word_count >= 3, f"malformed profile OpName at word {offset}")
            target = valid_id(operands[0], "profile OpName target")
            put_unique(names, target, reference.decode_literal_string(operands[1:], f"profile OpName at word {offset}"), "profile OpName target")
        elif opcode == 6:  # OpMemberName
            base.require(word_count >= 4, f"malformed profile OpMemberName at word {offset}")
            target = valid_id(operands[0], "profile OpMemberName target")
            put_unique(member_names, (target, operands[1]), reference.decode_literal_string(operands[2:], f"profile OpMemberName at word {offset}"), "profile OpMemberName target")
        elif opcode == 71:  # OpDecorate
            base.require(word_count >= 3, f"malformed profile OpDecorate at word {offset}")
            target = valid_id(operands[0], "profile OpDecorate target")
            put_unique(decorations, (target, operands[1]), tuple(operands[2:]), "profile OpDecorate target and kind")
        elif opcode == 72:  # OpMemberDecorate
            base.require(word_count >= 4, f"malformed profile OpMemberDecorate at word {offset}")
            target = valid_id(operands[0], "profile OpMemberDecorate target")
            put_unique(member_decorations, (target, operands[1], operands[2]), tuple(operands[3:]), "profile OpMemberDecorate target, member, and kind")
        elif opcode == 20:  # OpTypeBool
            base.require(word_count == 2, f"malformed OpTypeBool at word {offset}")
            put_type(unsupported_types, valid_id(operands[0], "OpTypeBool result"), "bool")
        elif opcode == 21:  # OpTypeInt
            base.require(word_count == 4, f"malformed OpTypeInt at word {offset}")
            result_id = valid_id(operands[0], "OpTypeInt result")
            base.require(operands[2] in {0, 1}, "profile integer signedness is invalid")
            put_type(scalar_types, result_id, (("i" if operands[2] else "u") + str(operands[1]), operands[1]))
        elif opcode == 22:  # OpTypeFloat
            base.require(word_count == 3, f"malformed OpTypeFloat at word {offset}")
            result_id = valid_id(operands[0], "OpTypeFloat result")
            put_type(scalar_types, result_id, (f"f{operands[1]}", operands[1]))
        elif opcode == 23:  # OpTypeVector
            base.require(word_count == 4, f"malformed OpTypeVector at word {offset}")
            put_type(vector_types, valid_id(operands[0], "OpTypeVector result"), (valid_id(operands[1], "OpTypeVector component"), operands[2]))
        elif opcode in {24, 28, 29}:  # matrix, array, runtime array
            labels = {24: "matrix", 28: "array", 29: "runtime-array"}
            base.require(word_count >= 3, f"malformed profile {labels[opcode]} type at word {offset}")
            put_type(unsupported_types, valid_id(operands[0], "unsupported profile type result"), labels[opcode])
        elif opcode == 30:  # OpTypeStruct
            base.require(word_count >= 2, f"malformed OpTypeStruct at word {offset}")
            put_type(struct_types, valid_id(operands[0], "OpTypeStruct result"), tuple(valid_id(value, "OpTypeStruct member") for value in operands[1:]))
        elif opcode == 32:  # OpTypePointer
            base.require(word_count == 4, f"malformed OpTypePointer at word {offset}")
            put_type(pointer_types, valid_id(operands[0], "OpTypePointer result"), (operands[1], valid_id(operands[2], "OpTypePointer pointee")))
        elif opcode == 59:  # OpVariable
            base.require(word_count in {4, 5}, f"malformed OpVariable at word {offset}")
            result_id = valid_id(operands[1], "OpVariable result")
            base.require(result_id not in defined_results, "duplicate profile result id")
            defined_results.add(result_id)
            variables[result_id] = (valid_id(operands[0], "OpVariable type"), operands[2])
        offset += word_count
    base.require(offset == len(words), "profile SPIR-V stream did not terminate exactly")

    push_globals = [(variable_id, type_id) for variable_id, (type_id, storage) in variables.items() if storage == 9]
    base.require(len(push_globals) <= 1, "multiple PushConstant globals are not reviewed")
    if not push_globals:
        return {"profile": validator_profile("baseline"), "immediate_witness": None}

    variable_id, pointer_id = push_globals[0]
    pointer = pointer_types.get(pointer_id)
    base.require(pointer is not None and pointer[0] == 9, "PushConstant variable does not use a PushConstant pointer")
    struct_id = pointer[1]
    members = struct_types.get(struct_id)
    base.require(members is not None, "PushConstant pointer does not point directly to one struct")
    base.require(decorations.get((struct_id, 2)) == (), "PushConstant struct lacks its exact Block decoration")
    base.require((struct_id, 3) not in decorations, "PushConstant struct also has BufferBlock decoration")
    base.require(variable_id in names and names[variable_id], "PushConstant variable name is missing")
    base.require(struct_id in names and names[struct_id], "PushConstant Block struct name is missing")
    base.require(members, "empty PushConstant Block struct is not reviewed")
    witness_members = []
    previous_end = 0
    content_extent = 0
    struct_alignment = 1
    for index, type_id in enumerate(members):
        name = member_names.get((struct_id, index))
        base.require(isinstance(name, str) and name, f"PushConstant member {index} name is missing")
        decoration = member_decorations.get((struct_id, index, 35))
        base.require(decoration is not None and len(decoration) == 1, f"PushConstant member {index} lacks one exact Offset")
        member_offset = decoration[0]
        if type_id in scalar_types:
            scalar_name, width = scalar_types[type_id]
            base.require(width == 32, f"PushConstant member {index} uses unsupported scalar width {width}")
            type_name = {"f32": "float", "u32": "uint", "i32": "int"}[scalar_name]
            size = 4
            alignment = 4
        elif type_id in vector_types:
            component_id, count = vector_types[type_id]
            base.require(component_id in scalar_types, f"PushConstant member {index} vector component type is unsupported")
            component_name, width = scalar_types[component_id]
            base.require(width == 32 and 2 <= count <= 4, f"PushConstant member {index} vector shape is unsupported")
            component_name = {"f32": "float", "u32": "uint", "i32": "int"}[component_name]
            type_name = f"{component_name}{count}"
            size = 4 * count
            alignment = 8 if count == 2 else 16
        else:
            kind = unsupported_types.get(type_id, "nested-or-recursive-or-unknown")
            raise base.ArtifactError(f"PushConstant member {index} uses unsupported {kind} type")
        end = member_offset + size
        base.require(end <= 0xFFFFFFFF, f"PushConstant member {index} extent overflows u32")
        base.require(member_offset >= previous_end, "PushConstant members overlap or are out of declaration order")
        previous_end = end
        content_extent = max(content_extent, end)
        struct_alignment = max(struct_alignment, alignment)
        witness_members.append({
            "index": index, "name": name, "type": type_name,
            "offset": member_offset, "size": size,
        })
    rounded_extent = content_extent + struct_alignment - 1
    base.require(rounded_extent <= 0xFFFFFFFF, "PushConstant struct alignment round-up overflows u32")
    occupied_extent = (rounded_extent // struct_alignment) * struct_alignment
    base.require(occupied_extent in PROFILE_EXTENTS, f"unreviewed PushConstant occupied extent: {occupied_extent}")
    profile_name = f"immediates-{occupied_extent}"
    witness = {
        "schema": IMMEDIATE_WITNESS_SCHEMA,
        "storage_class": "PushConstant",
        "variable_id": variable_id,
        "variable_name": names[variable_id],
        "pointer_type_id": pointer_id,
        "struct_type_id": struct_id,
        "struct_name": names[struct_id],
        "block": True,
        "members": witness_members,
        "required_max_immediate_size": occupied_extent,
    }
    return {"profile": validator_profile(profile_name), "immediate_witness": witness}


def validate_profile_derivation_record(derived: object, label: str) -> None:
    base.require_keys(derived, {"profile", "immediate_witness"}, label)
    witness = derived["immediate_witness"]
    if witness is None:
        base.require(derived["profile"] == validator_profile("baseline"), f"{label} baseline profile changed")
        return
    base.require_keys(witness, {
        "schema", "storage_class", "variable_id", "variable_name", "pointer_type_id",
        "struct_type_id", "struct_name", "block", "members", "required_max_immediate_size",
    }, label)
    base.require(witness["schema"] == IMMEDIATE_WITNESS_SCHEMA, f"{label} schema changed")
    for key in ("variable_id", "pointer_type_id", "struct_type_id"):
        base.require(isinstance(witness[key], int) and not isinstance(witness[key], bool) and witness[key] > 0, f"{label} {key} is invalid")
    base.require(isinstance(witness["variable_name"], str) and witness["variable_name"], f"{label} variable name is invalid")
    base.require(isinstance(witness["struct_name"], str) and witness["struct_name"], f"{label} struct name is invalid")
    base.require(witness["storage_class"] == "PushConstant" and witness["block"] is True, f"{label} boundary changed")
    members = witness["members"]
    base.require(isinstance(members, list) and members, f"{label} members are absent")
    intervals = []
    for index, member in enumerate(members):
        base.require_keys(member, {"index", "name", "type", "offset", "size"}, f"{label} member {index}")
        base.require(member["index"] == index, f"{label} member order changed")
        base.require(isinstance(member["name"], str) and member["name"], f"{label} member name changed")
        allowed_types = {
            "float": (4, 4), "uint": (4, 4), "int": (4, 4),
            "float2": (8, 8), "uint2": (8, 8), "int2": (8, 8),
            "float3": (12, 16), "uint3": (12, 16), "int3": (12, 16),
            "float4": (16, 16), "uint4": (16, 16), "int4": (16, 16),
        }
        base.require(isinstance(member["type"], str) and member["type"] in allowed_types, f"{label} member type changed")
        base.require(isinstance(member["offset"], int) and not isinstance(member["offset"], bool) and member["offset"] >= 0, f"{label} member offset changed")
        size, alignment = allowed_types[member["type"]]
        base.require(member["size"] == size, f"{label} member size changed")
        member_end = member["offset"] + member["size"]
        base.require(member_end <= 0xFFFFFFFF, f"{label} member extent overflowed")
        intervals.append((member["offset"], member_end, alignment))
    base.require(all(left[1] <= right[0] for left, right in zip(intervals, intervals[1:])), f"{label} members overlap or changed order")
    content_extent = max(end for _, end, _ in intervals)
    struct_alignment = max(alignment for _, _, alignment in intervals)
    rounded_extent = content_extent + struct_alignment - 1
    base.require(rounded_extent <= 0xFFFFFFFF, f"{label} alignment round-up overflowed")
    extent = (rounded_extent // struct_alignment) * struct_alignment
    base.require(extent == witness["required_max_immediate_size"] and extent in PROFILE_EXTENTS, f"{label} occupied extent changed")
    base.require(derived["profile"] == validator_profile(f"immediates-{extent}"), f"{label} selected profile changed")


def scalar_layout_witness(artifact_bytes: bytes, policy: dict) -> dict | None:
    base.require(len(artifact_bytes) >= 20 and len(artifact_bytes) % 4 == 0, "scalar-layout SPIR-V extent is invalid")
    words = list(struct.unpack(f"<{len(artifact_bytes) // 4}I", artifact_bytes))
    base.require(words[0] == 0x07230203 and words[3] >= 1 and words[4] == 0, "scalar-layout SPIR-V header is invalid")
    bound = words[3]
    names: dict[int, str] = {}
    member_names: dict[tuple[int, int], str] = {}
    decorations: dict[tuple[int, int], tuple[int, ...]] = {}
    member_decorations: dict[tuple[int, int, int], tuple[int, ...]] = {}
    float_types: dict[int, int] = {}
    vector_types: dict[int, tuple[int, int]] = {}
    runtime_arrays: dict[int, int] = {}
    struct_types: dict[int, tuple[int, ...]] = {}
    pointer_types: dict[int, tuple[int, int]] = {}
    variables: dict[int, tuple[int, int]] = {}

    def put_unique(table: dict, key: object, value: object, label: str) -> None:
        base.require(key not in table, f"duplicate {label}")
        table[key] = value

    def valid_id(value: int, label: str) -> int:
        base.require(0 < value < bound, f"{label} is outside the SPIR-V id bound")
        return value

    offset = 5
    while offset < len(words):
        first = words[offset]
        word_count = first >> 16
        opcode = first & 0xFFFF
        base.require(word_count > 0 and offset + word_count <= len(words), f"malformed scalar-layout SPIR-V instruction at word {offset}")
        operands = words[offset + 1 : offset + word_count]
        if opcode in {73, 74, 75}:
            raise base.ArtifactError(f"scalar-layout group decoration is not implemented at word {offset}")
        if opcode == 5:  # OpName
            base.require(word_count >= 3, f"malformed OpName at word {offset}")
            target = valid_id(operands[0], "OpName target")
            put_unique(names, target, reference.decode_literal_string(operands[1:], f"OpName at word {offset}"), "OpName target")
        elif opcode == 6:  # OpMemberName
            base.require(word_count >= 4, f"malformed OpMemberName at word {offset}")
            target = valid_id(operands[0], "OpMemberName target")
            put_unique(member_names, (target, operands[1]), reference.decode_literal_string(operands[2:], f"OpMemberName at word {offset}"), "OpMemberName target")
        elif opcode == 71:  # OpDecorate
            base.require(word_count >= 3, f"malformed scalar-layout OpDecorate at word {offset}")
            target = valid_id(operands[0], "OpDecorate target")
            put_unique(decorations, (target, operands[1]), tuple(operands[2:]), "OpDecorate target and kind")
        elif opcode == 72:  # OpMemberDecorate
            base.require(word_count >= 4, f"malformed scalar-layout OpMemberDecorate at word {offset}")
            target = valid_id(operands[0], "OpMemberDecorate target")
            put_unique(member_decorations, (target, operands[1], operands[2]), tuple(operands[3:]), "OpMemberDecorate target, member, and kind")
        elif opcode == 22:  # OpTypeFloat
            base.require(word_count == 3, f"malformed OpTypeFloat at word {offset}")
            put_unique(float_types, valid_id(operands[0], "OpTypeFloat result"), operands[1], "OpTypeFloat result")
        elif opcode == 23:  # OpTypeVector
            base.require(word_count == 4, f"malformed OpTypeVector at word {offset}")
            put_unique(vector_types, valid_id(operands[0], "OpTypeVector result"), (valid_id(operands[1], "OpTypeVector component"), operands[2]), "OpTypeVector result")
        elif opcode == 29:  # OpTypeRuntimeArray
            base.require(word_count == 3, f"malformed OpTypeRuntimeArray at word {offset}")
            put_unique(runtime_arrays, valid_id(operands[0], "OpTypeRuntimeArray result"), valid_id(operands[1], "OpTypeRuntimeArray element"), "OpTypeRuntimeArray result")
        elif opcode == 30:  # OpTypeStruct
            base.require(word_count >= 2, f"malformed OpTypeStruct at word {offset}")
            put_unique(struct_types, valid_id(operands[0], "OpTypeStruct result"), tuple(valid_id(value, "OpTypeStruct member") for value in operands[1:]), "OpTypeStruct result")
        elif opcode == 32:  # OpTypePointer
            base.require(word_count == 4, f"malformed OpTypePointer at word {offset}")
            put_unique(pointer_types, valid_id(operands[0], "OpTypePointer result"), (operands[1], valid_id(operands[2], "OpTypePointer pointee")), "OpTypePointer result")
        elif opcode == 59:  # OpVariable
            base.require(word_count in {4, 5}, f"malformed OpVariable at word {offset}")
            put_unique(variables, valid_id(operands[1], "OpVariable result"), (valid_id(operands[0], "OpVariable type"), operands[2]), "OpVariable result")
        offset += word_count
    base.require(offset == len(words), "scalar-layout SPIR-V stream did not terminate exactly")

    matching = [identifier for identifier, name in names.items() if name == "instanceRDPParams"]
    if not matching:
        return None
    base.require(len(matching) == 1, "scalar-layout witness variable name is ambiguous")
    variable_id = matching[0]
    base.require(variable_id in variables, "scalar-layout witness name does not identify a variable")
    pointer_id, storage_class = variables[variable_id]
    base.require(storage_class == 2, "scalar-layout witness storage class changed")
    base.require(pointer_types.get(pointer_id, (None, None))[0] == 2, "scalar-layout witness pointer storage class changed")
    container_id = pointer_types[pointer_id][1]
    base.require(names.get(container_id) == "type.StructuredBuffer.RDPParams", "scalar-layout witness container name changed")
    base.require(struct_types.get(container_id) is not None and len(struct_types[container_id]) == 1, "scalar-layout witness container shape changed")
    runtime_array_id = struct_types[container_id][0]
    rdp_params_id = runtime_arrays.get(runtime_array_id)
    base.require(rdp_params_id is not None, "scalar-layout witness runtime array changed")
    base.require(names.get(rdp_params_id) == "RDPParams", "scalar-layout witness struct name changed")
    members = struct_types.get(rdp_params_id)
    base.require(members is not None and len(members) > 7, "scalar-layout witness member denominator changed")
    key_scale_type = members[7]
    float_type, vector_length = vector_types.get(key_scale_type, (None, None))
    base.require(vector_length == 3 and float_types.get(float_type) == 32, "scalar-layout witness member type changed")

    def exact_decoration(target: int, decoration: int, expected: tuple[int, ...], label: str) -> None:
        base.require(decorations.get((target, decoration)) == expected, f"scalar-layout witness {label} changed")

    def exact_member_decoration(target: int, member: int, decoration: int, expected: tuple[int, ...], label: str) -> None:
        base.require(member_decorations.get((target, member, decoration)) == expected, f"scalar-layout witness {label} changed")

    exact_decoration(variable_id, 34, (0,), "descriptor set")
    exact_decoration(variable_id, 33, (2,), "binding")
    exact_decoration(container_id, 3, (), "BufferBlock decoration")
    exact_decoration(runtime_array_id, 6, (128,), "runtime-array stride")
    exact_member_decoration(container_id, 0, 35, (0,), "container member offset")
    exact_member_decoration(container_id, 0, 24, (), "container member read-only decoration")
    exact_member_decoration(rdp_params_id, 7, 35, (92,), "keyScale member offset")
    base.require(member_names.get((rdp_params_id, 7)) == "keyScale", "scalar-layout witness member name changed")
    required_alignment = 4 * (float_types[float_type] // 8)
    fields = {
        "schema": "fn64.spirv-scalar-layout-witness.v1", "variable_name": names[variable_id],
        "storage_class": "Uniform", "buffer_block": True, "descriptor_set": 0, "binding": 2,
        "container_name": names[container_id], "runtime_array_stride": 128,
        "struct_name": names[rdp_params_id], "member_index": 7,
        "member_name": member_names[(rdp_params_id, 7)], "member_type": "float3",
        "member_offset": 92, "required_alignment": required_alignment,
        "offset_aligned": 92 % required_alignment == 0,
    }
    base.require(fields == policy["outcomes"]["blocked_known_scalar_layout"]["witness"], "scalar-layout witness contract changed")
    fields["witness_sha256"] = base.digest_bytes(base.canonical_json(fields))
    return fields


def sampled_buffer_witness(artifact_bytes: bytes, policy: dict) -> dict | None:
    base.require(len(artifact_bytes) >= 20 and len(artifact_bytes) % 4 == 0, "sampled-buffer SPIR-V extent is invalid")
    words = list(struct.unpack(f"<{len(artifact_bytes) // 4}I", artifact_bytes))
    base.require(words[0] == 0x07230203 and words[3] >= 1 and words[4] == 0, "sampled-buffer SPIR-V header is invalid")
    expected = policy["outcomes"]["blocked_known_sampled_buffer"]["required_capability"]
    matches: list[int] = []
    offset = 5
    while offset < len(words):
        first = words[offset]
        word_count = first >> 16
        opcode = first & 0xFFFF
        base.require(word_count > 0 and offset + word_count <= len(words), f"malformed sampled-buffer SPIR-V instruction at word {offset}")
        operands = words[offset + 1 : offset + word_count]
        if opcode in {73, 74, 75}:
            raise base.ArtifactError(f"sampled-buffer group decoration is not implemented at word {offset}")
        if opcode == 17:  # OpCapability
            base.require(word_count == 2, f"malformed OpCapability at word {offset}")
            if operands[0] == expected["value"]:
                matches.append(offset)
        offset += word_count
    base.require(offset == len(words), "sampled-buffer SPIR-V stream did not terminate exactly")
    if not matches:
        return None
    base.require(len(matches) == 1, "sampled-buffer witness capability is not declared exactly once")
    fields = {
        "schema": "fn64.spirv-sampled-buffer-witness.v1",
        "capability": {"name": expected["name"], "value": expected["value"]},
        "word_offset": matches[0],
    }
    fields["witness_sha256"] = base.digest_bytes(base.canonical_json(fields))
    return fields


def validate_sampled_buffer_witness_record(witness: object, policy: dict, label: str) -> None:
    expected = policy["outcomes"]["blocked_known_sampled_buffer"]["required_capability"]
    base.require_keys(witness, {"schema", "capability", "word_offset", "witness_sha256"}, label)
    unhashed = copy.deepcopy(witness)
    digest = require_sha(unhashed.pop("witness_sha256", None), f"{label} identity")
    base.require(unhashed["schema"] == "fn64.spirv-sampled-buffer-witness.v1", f"{label} schema changed")
    base.require(unhashed["capability"] == expected, f"{label} capability changed")
    base.require(isinstance(unhashed["word_offset"], int) and not isinstance(unhashed["word_offset"], bool) and unhashed["word_offset"] >= 5, f"{label} word offset is invalid")
    base.require(digest == base.digest_bytes(base.canonical_json(unhashed)), f"{label} identity changed")


def fragment_interface_witness(artifact_bytes: bytes, stage: str, entry: str, policy: dict) -> dict | None:
    base.require(len(artifact_bytes) >= 20 and len(artifact_bytes) % 4 == 0, "fragment-interface SPIR-V extent is invalid")
    words = list(struct.unpack(f"<{len(artifact_bytes) // 4}I", artifact_bytes))
    base.require(words[0] == 0x07230203 and words[3] >= 1 and words[4] == 0, "fragment-interface SPIR-V header is invalid")
    bound = words[3]
    expected = policy["outcomes"]["blocked_known_fragment_direct_blend_src_index_output"]["witness"]
    if stage != expected["stage"] or entry != expected["entry"]:
        return None

    def valid_id(value: int, label: str) -> int:
        base.require(0 < value < bound, f"{label} is outside the SPIR-V id bound")
        return value

    def put_unique(table: dict, key: object, value: object, label: str) -> None:
        base.require(key not in table, f"duplicate {label}")
        table[key] = value

    def decode_string(operand_words: tuple[int, ...], label: str) -> str:
        return reference.decode_literal_string(operand_words, label)

    names: dict[int, str] = {}
    decorations: dict[tuple[int, int], tuple[int, ...]] = {}
    float_types: dict[int, int] = {}
    vector_types: dict[int, tuple[int, int]] = {}
    pointer_types: dict[int, tuple[int, int]] = {}
    variables: dict[int, tuple[int, int]] = {}
    entry_points: list[tuple[int, str, tuple[int, ...]]] = []

    offset = 5
    while offset < len(words):
        first = words[offset]
        word_count = first >> 16
        opcode = first & 0xFFFF
        base.require(word_count > 0 and offset + word_count <= len(words), f"malformed fragment-interface SPIR-V instruction at word {offset}")
        operands = words[offset + 1 : offset + word_count]
        if opcode in {73, 74, 75}:
            raise base.ArtifactError(f"fragment-interface group decoration is not implemented at word {offset}")
        if opcode == 15:  # OpEntryPoint
            base.require(word_count >= 3, f"malformed OpEntryPoint at word {offset}")
            execution_model = operands[0]
            entry_id = valid_id(operands[1], "OpEntryPoint function")
            rest = operands[2:]
            name_words: list[int] = []
            for word in rest:
                name_words.append(word)
                if any(byte == 0 for byte in word.to_bytes(4, "little")):
                    break
            entry_name = decode_string(tuple(name_words), f"OpEntryPoint at word {offset}")
            consumed = len(name_words)
            interface = tuple(valid_id(value, "OpEntryPoint interface") for value in rest[consumed:])
            if execution_model == 4 and entry_name == entry:
                entry_points.append((entry_id, entry_name, interface))
        elif opcode == 5:  # OpName
            base.require(word_count >= 3, f"malformed OpName at word {offset}")
            target = valid_id(operands[0], "OpName target")
            put_unique(names, target, decode_string(operands[1:], f"OpName at word {offset}"), "OpName target")
        elif opcode == 71:  # OpDecorate
            base.require(word_count >= 3, f"malformed OpDecorate at word {offset}")
            target = valid_id(operands[0], "OpDecorate target")
            put_unique(decorations, (target, operands[1]), tuple(operands[2:]), "OpDecorate target and kind")
        elif opcode == 22:  # OpTypeFloat
            base.require(word_count == 3, f"malformed OpTypeFloat at word {offset}")
            put_unique(float_types, valid_id(operands[0], "OpTypeFloat result"), operands[1], "OpTypeFloat result")
        elif opcode == 23:  # OpTypeVector
            base.require(word_count == 4, f"malformed OpTypeVector at word {offset}")
            put_unique(vector_types, valid_id(operands[0], "OpTypeVector result"), (valid_id(operands[1], "OpTypeVector component"), operands[2]), "OpTypeVector result")
        elif opcode == 32:  # OpTypePointer
            base.require(word_count == 4, f"malformed OpTypePointer at word {offset}")
            put_unique(pointer_types, valid_id(operands[0], "OpTypePointer result"), (operands[1], valid_id(operands[2], "OpTypePointer pointee")), "OpTypePointer result")
        elif opcode == 59:  # OpVariable
            base.require(word_count in {4, 5}, f"malformed OpVariable at word {offset}")
            put_unique(variables, valid_id(operands[1], "OpVariable result"), (valid_id(operands[0], "OpVariable type"), operands[2]), "OpVariable result")
        offset += word_count
    base.require(offset == len(words), "fragment-interface SPIR-V stream did not terminate exactly")

    if not entry_points:
        return None
    base.require(len(entry_points) == 1, "fragment-interface witness entry point is ambiguous")
    _, _, interface = entry_points[0]

    matching = [identifier for identifier, name in names.items() if name == expected["variable_name"]]
    if not matching:
        return None
    base.require(len(matching) == 1, "fragment-interface witness variable name is ambiguous")
    variable_id = matching[0]
    base.require(variable_id in interface, "fragment-interface witness variable is not a direct entry-point interface member")
    base.require(variable_id in variables, "fragment-interface witness name does not identify a variable")
    pointer_id, storage_class = variables[variable_id]
    base.require(storage_class == 3, "fragment-interface witness storage class changed")  # StorageClass Output
    pointer = pointer_types.get(pointer_id)
    base.require(pointer is not None and pointer[0] == 3, "fragment-interface witness pointer storage class changed")
    value_type = pointer[1]
    component_id, vector_length = vector_types.get(value_type, (None, None))
    base.require(component_id is not None and vector_length == 4 and float_types.get(component_id) == 32, "fragment-interface witness member type changed")
    base.require(decorations.get((variable_id, 30)) == (expected["location"],), "fragment-interface witness location changed")
    base.require(decorations.get((variable_id, 43)) == (expected["index"],), "fragment-interface witness index changed")
    fields = {
        "schema": "fn64.spirv-fragment-blend-src-index-output-witness.v1",
        "stage": stage, "entry": entry, "variable_name": names[variable_id],
        "storage_class": "Output", "type": "float4", "direct_interface_member": True,
        "location": expected["location"], "index": expected["index"],
    }
    base.require(fields == expected, "fragment-interface witness contract changed")
    fields["witness_sha256"] = base.digest_bytes(base.canonical_json(fields))
    return fields


def validate_fragment_interface_witness_record(witness: object, policy: dict, label: str) -> None:
    expected = policy["outcomes"]["blocked_known_fragment_direct_blend_src_index_output"]["witness"]
    base.require_keys(witness, set(expected) | {"witness_sha256"}, label)
    unhashed = copy.deepcopy(witness)
    digest = require_sha(unhashed.pop("witness_sha256", None), f"{label} identity")
    base.require(unhashed == expected, f"{label} fields changed")
    base.require(digest == base.digest_bytes(base.canonical_json(unhashed)), f"{label} identity changed")


def inventory_supports_known_blocker(inventory: object, policy: dict) -> None:
    base.require(isinstance(inventory, dict), "blocked-known row has no semantic inventory")
    blocked = policy["outcomes"]["blocked_known_shader_nonuniform"]
    capabilities = inventory.get("capabilities")
    extensions = inventory.get("extensions")
    decorations = inventory.get("non_uniform_decorations")
    base.require(isinstance(capabilities, list) and blocked["required_capability"] in [
        {"name": row.get("name"), "value": row.get("value")} for row in capabilities if isinstance(row, dict)
    ], "blocked-known row lacks exact ShaderNonUniform capability")
    base.require(isinstance(extensions, list) and blocked["required_extension"] in [
        row.get("name") for row in extensions if isinstance(row, dict)
    ], "blocked-known row lacks SPV_EXT_descriptor_indexing")
    base.require(isinstance(decorations, list) and len(decorations) > 0, "blocked-known row lacks a direct NonUniform decoration")


def validate_inventory_record(inventory: object, label: str) -> None:
    base.require_keys(inventory, {
        "schema", "word_count", "id_bound", "capabilities", "extensions",
        "non_uniform_decorations", "inventory_sha256",
    }, label)
    base.require(inventory["schema"] == "fn64.spirv-semantic-inventory.v1", f"{label} schema changed")
    base.require(isinstance(inventory["word_count"], int) and inventory["word_count"] >= 5, f"{label} word count is invalid")
    base.require(isinstance(inventory["id_bound"], int) and inventory["id_bound"] >= 1, f"{label} id bound is invalid")
    for index, row in enumerate(inventory["capabilities"]):
        base.require_keys(row, {"name", "value", "word_offset"}, f"{label} capability {index}")
        base.require(isinstance(row["name"], str) and isinstance(row["value"], int) and isinstance(row["word_offset"], int), f"{label} capability {index} is malformed")
    for index, row in enumerate(inventory["extensions"]):
        base.require_keys(row, {"name", "word_offset"}, f"{label} extension {index}")
        base.require(isinstance(row["name"], str) and isinstance(row["word_offset"], int), f"{label} extension {index} is malformed")
    for index, row in enumerate(inventory["non_uniform_decorations"]):
        base.require_keys(row, {"target_id", "word_offset"}, f"{label} NonUniform decoration {index}")
        base.require(isinstance(row["target_id"], int) and 0 < row["target_id"] < inventory["id_bound"], f"{label} NonUniform target is invalid")
        base.require(isinstance(row["word_offset"], int), f"{label} NonUniform offset is invalid")
    unhashed = copy.deepcopy(inventory)
    digest = require_sha(unhashed.pop("inventory_sha256", None), f"{label} identity")
    base.require(digest == base.digest_bytes(base.canonical_json(unhashed)), f"{label} identity changed")


def validate_scalar_witness_record(witness: object, policy: dict, label: str) -> None:
    expected = policy["outcomes"]["blocked_known_scalar_layout"]["witness"]
    base.require_keys(witness, set(expected) | {"witness_sha256"}, label)
    unhashed = copy.deepcopy(witness)
    digest = require_sha(unhashed.pop("witness_sha256", None), f"{label} identity")
    base.require(unhashed == expected, f"{label} fields changed")
    base.require(digest == base.digest_bytes(base.canonical_json(unhashed)), f"{label} identity changed")


def classify_result(result: subprocess.CompletedProcess[bytes], row: dict, artifact_bytes: bytes, policy: dict) -> tuple[str, str | None, dict | None, dict | None, dict | None, dict | None]:
    stdout_sha = base.digest_bytes(result.stdout)
    stderr_sha = base.digest_bytes(result.stderr)
    inventory = row.get("semantic_inventory")
    has_shader_nonuniform = isinstance(inventory, dict) and policy["outcomes"]["blocked_known_shader_nonuniform"]["required_capability"] in [
        {"name": item.get("name"), "value": item.get("value")}
        for item in inventory.get("capabilities", []) if isinstance(item, dict)
    ]
    profile_derivation = derive_validator_profile(artifact_bytes, policy)
    profile_name = profile_derivation["profile"]["name"]
    scalar_witness = scalar_layout_witness(artifact_bytes, policy)
    buffer_witness = sampled_buffer_witness(artifact_bytes, policy)
    fragment_witness = fragment_interface_witness(artifact_bytes, row["stage"], row["entry"], policy)
    ingestible = policy["outcomes"]["ingestible"]
    if result.returncode == ingestible["exit_code"] and stderr_sha == ingestible["stderr_sha256"]:
        base.require(not has_shader_nonuniform, f"ShaderNonUniform witness unexpectedly passed strict wgpu ingestion: {row['id']}")
        base.require(scalar_witness is None, f"scalar-layout witness unexpectedly passed strict wgpu ingestion: {row['id']}")
        base.require(buffer_witness is None, f"sampled-buffer witness unexpectedly passed strict wgpu ingestion: {row['id']}")
        base.require(fragment_witness is None, f"fragment-interface witness unexpectedly passed strict wgpu ingestion: {row['id']}")
        try:
            record = json.loads(result.stdout)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise base.ArtifactError(f"ingestible row emitted malformed JSON: {row['id']}: {error}") from error
        base.require_keys(record, {"schema", "status", "wgpu_major", "profile", "stage", "entry", "module_bytes"}, f"ingestible stdout {row['id']}")
        expected_record = validator_success_record(profile_name, row["stage"], row["entry"], len(artifact_bytes))
        base.require(record == expected_record, f"ingestible stdout changed: {row['id']}")
        expected_bytes = validator_success_bytes(profile_name, row["stage"], row["entry"], len(artifact_bytes))
        base.require(result.stdout == expected_bytes, f"ingestible stdout bytes changed: {row['id']}")
        return "ingestible", None, record, None, None, None
    blocked = policy["outcomes"]["blocked_known_shader_nonuniform"]
    if result.returncode == blocked["exit_code"] and stdout_sha == blocked["stdout_sha256"] and stderr_sha == blocked["stderr_sha256"]:
        base.require(result.stderr == KNOWN_STDERR, f"blocked-known stderr bytes changed: {row['id']}")
        inventory_supports_known_blocker(row.get("semantic_inventory"), policy)
        return "blocked-known", blocked["reason_code"], None, scalar_witness, buffer_witness, fragment_witness
    scalar = policy["outcomes"]["blocked_known_scalar_layout"]
    if result.returncode == scalar["exit_code"] and stdout_sha == scalar["stdout_sha256"] and stderr_sha == scalar["stderr_sha256"]:
        base.require(result.stderr == SCALAR_LAYOUT_STDERR, f"blocked-known scalar-layout stderr bytes changed: {row['id']}")
        base.require(scalar_witness is not None, f"blocked-known scalar-layout row lacks its exact structural witness: {row['id']}")
        return "blocked-known", scalar["reason_code"], None, scalar_witness, buffer_witness, fragment_witness
    sampled_buffer = policy["outcomes"]["blocked_known_sampled_buffer"]
    if result.returncode == sampled_buffer["exit_code"] and stdout_sha == sampled_buffer["stdout_sha256"] and stderr_sha == sampled_buffer["stderr_sha256"]:
        base.require(result.stderr == SAMPLED_BUFFER_STDERR, f"blocked-known sampled-buffer stderr bytes changed: {row['id']}")
        base.require(buffer_witness is not None, f"blocked-known sampled-buffer row lacks its exact structural witness: {row['id']}")
        return "blocked-known", sampled_buffer["reason_code"], None, scalar_witness, buffer_witness, fragment_witness
    fragment_interface = policy["outcomes"]["blocked_known_fragment_direct_blend_src_index_output"]
    if result.returncode == fragment_interface["exit_code"] and stdout_sha == fragment_interface["stdout_sha256"] and stderr_sha == fragment_interface["stderr_sha256"]:
        base.require(result.stderr == FRAGMENT_INTERFACE_STDERR, f"blocked-known fragment-interface stderr bytes changed: {row['id']}")
        base.require(fragment_witness is not None, f"blocked-known fragment-interface row lacks its exact structural witness: {row['id']}")
        return "blocked-known", fragment_interface["reason_code"], None, scalar_witness, buffer_witness, fragment_witness
    raise base.ArtifactError(
        f"unclassified wgpu validator outcome for {row['id']}: exit={result.returncode} stdout_sha256={stdout_sha} stderr_sha256={stderr_sha}"
    )


def assess_row(validator: Path, corpus_dir: Path, private_root: Path, row: dict, policy: dict) -> dict:
    base.require_keys(row, set(row), f"reference row {row.get('id', '<unknown>')}")
    for key in ("id", "source", "stage", "entry", "spirv_artifact", "spirv_sha256", "spirv_bytes", "semantic_inventory"):
        base.require(key in row, f"reference row lacks {key}")
    relative = safe_relative(row["spirv_artifact"], f"SPIR-V artifact {row['id']}")
    artifact = corpus_dir.joinpath(*relative.parts)
    artifact_bytes, info = base.stable_regular_bytes(artifact, policy["limits"]["maximum_artifact_bytes"], f"reference SPIR-V {row['id']}")
    base.require(info.st_nlink == 1, f"reference SPIR-V has another hardlink: {row['id']}")
    digest = base.digest_bytes(artifact_bytes)
    base.require(digest == require_sha(row["spirv_sha256"], f"reference SPIR-V digest {row['id']}") and len(artifact_bytes) == row["spirv_bytes"], f"reference SPIR-V identity changed: {row['id']}")
    row_dir = private_root / "rows" / row["id"]
    base.require(row_dir.parent == private_root / "rows" and row["id"].replace("-", "").isalnum(), f"unsafe reference row id: {row['id']}")
    row_dir.mkdir(mode=0o700, parents=True)
    snapshot = row_dir / "input.spv"
    base.write_new_private_file(snapshot, artifact_bytes)
    profile_derivation = derive_validator_profile(artifact_bytes, policy)
    selected_profile = profile_derivation["profile"]
    arguments = [
        str(validator), "--profile", selected_profile["name"], "--shader", str(snapshot),
        "--stage", row["stage"], "--entry", row["entry"],
    ]
    before_files = sorted(path.relative_to(private_root).as_posix() for path in private_root.rglob("*") if path.is_file() or path.is_symlink())
    result = subprocess.run(
        arguments, cwd=private_root, env=controlled_environment(private_root, policy),
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        timeout=policy["limits"]["validator_timeout_seconds"], check=False, close_fds=True,
    )
    base.require(len(result.stdout) <= policy["limits"]["maximum_process_output_bytes"], f"wgpu validator stdout exceeded policy limit: {row['id']}")
    base.require(len(result.stderr) <= policy["limits"]["maximum_process_output_bytes"], f"wgpu validator stderr exceeded policy limit: {row['id']}")
    after_files = sorted(path.relative_to(private_root).as_posix() for path in private_root.rglob("*") if path.is_file() or path.is_symlink())
    base.require(before_files == after_files, f"wgpu validator changed the private file set: {row['id']}")
    base.require(base.digest_file(snapshot) == digest and base.digest_file(validator) == policy["wgpu_validator"]["binary_sha256"], f"wgpu validator changed qualified bytes: {row['id']}")
    outcome, reason, stdout_record, scalar_witness, buffer_witness, fragment_witness = classify_result(result, row, artifact_bytes, policy)
    transcript = {
        "arguments": validator_arguments(selected_profile["name"], row["stage"], row["entry"]),
        "exit_code": result.returncode, "stdout_sha256": base.digest_bytes(result.stdout),
        "stdout_bytes": len(result.stdout), "stderr_sha256": base.digest_bytes(result.stderr),
        "stderr_bytes": len(result.stderr),
    }
    return {
        "id": row["id"], "source": row["source"], "stage": row["stage"], "entry": row["entry"],
        "spirv_artifact": row["spirv_artifact"], "spirv_sha256": digest, "spirv_bytes": len(artifact_bytes),
        "semantic_inventory": copy.deepcopy(row["semantic_inventory"]),
        "immediate_witness": profile_derivation["immediate_witness"], "selected_profile": selected_profile,
        "outcome": outcome,
        "scalar_layout_witness": scalar_witness, "sampled_buffer_witness": buffer_witness,
        "fragment_interface_witness": fragment_witness, "reason_code": reason,
        "validation": transcript, "validation_record": stdout_record,
    }


def diagnostic_text(value: bytes, exit_code: int | None, policy: dict) -> str | None:
    maximum = policy["diagnostic_census"]["maximum_text_bytes"]
    if exit_code == 0 or len(value) > maximum:
        return None
    try:
        decoded = value.decode("utf-8")
    except UnicodeDecodeError:
        return None
    if base.LOCAL_PATH_RE.search(decoded) or any(not (character.isprintable() or character in "\n\r\t") for character in decoded):
        return None
    return decoded


def _set_diagnostic_output_limit(maximum: int) -> None:
    assert resource is not None
    resource.setrlimit(resource.RLIMIT_FSIZE, (maximum, maximum))


def create_private_output_file(path: Path) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags, 0o600)
    os.close(descriptor)
    info = path.lstat()
    base.require(stat.S_ISREG(info.st_mode) and not path.is_symlink() and info.st_nlink == 1, f"diagnostic output is not a private regular file: {path.name}")


def diagnose_row(validator: Path, corpus_dir: Path, private_root: Path, row: dict, policy: dict) -> dict:
    relative = safe_relative(row.get("spirv_artifact"), f"diagnostic SPIR-V artifact {row.get('id')}")
    artifact = corpus_dir.joinpath(*relative.parts)
    artifact_bytes, info = base.stable_regular_bytes(
        artifact, policy["limits"]["maximum_artifact_bytes"], f"diagnostic reference SPIR-V {row.get('id')}"
    )
    base.require(info.st_nlink == 1, f"diagnostic reference SPIR-V has another hardlink: {row.get('id')}")
    digest = base.digest_bytes(artifact_bytes)
    base.require(
        digest == require_sha(row.get("spirv_sha256"), f"diagnostic SPIR-V digest {row.get('id')}")
        and len(artifact_bytes) == row.get("spirv_bytes"),
        f"diagnostic reference SPIR-V identity changed: {row.get('id')}",
    )
    profile_derivation = derive_validator_profile(artifact_bytes, policy)
    selected_profile = profile_derivation["profile"]
    row_dir = private_root / "diagnostic-rows" / row["id"]
    base.require(row_dir.parent == private_root / "diagnostic-rows" and row["id"].replace("-", "").isalnum(), f"unsafe diagnostic row id: {row['id']}")
    row_dir.mkdir(mode=0o700, parents=True)
    snapshot = row_dir / "input.spv"
    stdout_path = row_dir / "stdout"
    stderr_path = row_dir / "stderr"
    base.write_new_private_file(snapshot, artifact_bytes)
    create_private_output_file(stdout_path)
    create_private_output_file(stderr_path)
    arguments = [
        str(validator), "--profile", selected_profile["name"], "--shader", str(snapshot),
        "--stage", row["stage"], "--entry", row["entry"],
    ]
    before_files = sorted(path.relative_to(private_root).as_posix() for path in private_root.rglob("*") if path.is_file() or path.is_symlink())
    stream_limit = policy["diagnostic_census"]["maximum_stream_bytes"]
    termination = "exited"
    exit_code: int | None
    with stdout_path.open("wb") as stdout_file, stderr_path.open("wb") as stderr_file:
        try:
            result = subprocess.run(
                arguments, cwd=private_root, env=controlled_environment(private_root, policy),
                stdin=subprocess.DEVNULL, stdout=stdout_file, stderr=stderr_file,
                timeout=policy["limits"]["validator_timeout_seconds"], check=False, close_fds=True,
                preexec_fn=lambda: _set_diagnostic_output_limit(stream_limit),
            )
            exit_code = result.returncode
        except subprocess.TimeoutExpired:
            termination = "timeout"
            exit_code = None
    after_files = sorted(path.relative_to(private_root).as_posix() for path in private_root.rglob("*") if path.is_file() or path.is_symlink())
    base.require(before_files == after_files, f"diagnostic validator changed the private file set: {row['id']}")
    stdout, stdout_info = base.stable_regular_bytes(stdout_path, stream_limit, f"diagnostic stdout {row['id']}")
    stderr, stderr_info = base.stable_regular_bytes(stderr_path, stream_limit, f"diagnostic stderr {row['id']}")
    base.require(stdout_info.st_nlink == 1 and stderr_info.st_nlink == 1, f"diagnostic output was linked: {row['id']}")
    row_output_bytes = len(stdout) + len(stderr)
    base.require(row_output_bytes <= policy["diagnostic_census"]["maximum_row_output_bytes"], f"diagnostic row output cap changed: {row['id']}")
    base.require(
        base.digest_file(snapshot) == digest and base.digest_file(validator) == policy["wgpu_validator"]["binary_sha256"],
        f"diagnostic validator changed qualified bytes: {row['id']}",
    )
    return {
        "id": row["id"], "source": row["source"], "stage": row["stage"], "entry": row["entry"],
        "spirv_artifact": row["spirv_artifact"], "spirv_sha256": digest, "spirv_bytes": len(artifact_bytes),
        "selected_profile": selected_profile, "immediate_witness": profile_derivation["immediate_witness"],
        "validation": {
            "arguments": validator_arguments(selected_profile["name"], row["stage"], row["entry"]),
            "termination": termination, "exit_code": exit_code,
            "stdout_bytes": len(stdout), "stdout_sha256": base.digest_bytes(stdout),
            "stderr_bytes": len(stderr), "stderr_sha256": base.digest_bytes(stderr),
            "stdout_text": diagnostic_text(stdout, exit_code, policy),
            "stderr_text": diagnostic_text(stderr, exit_code, policy),
        },
    }


def collect_diagnostic_rows(reference_rows: list[dict], runner, policy: dict) -> tuple[list[dict], dict]:
    census = policy["diagnostic_census"]
    base.require(len(reference_rows) == census["row_count"], "diagnostic census row denominator changed")
    entries = []
    total_output_bytes = 0
    exit_counts: dict[str, int] = {}
    selected_profile_counts = {name: 0 for name in PROFILE_NAMES}
    seen = set()
    for row in reference_rows:
        base.require(row.get("id") not in seen, "diagnostic census row id repeated")
        seen.add(row.get("id"))
        entry = runner(row)
        base.require(entry.get("id") == row.get("id"), "diagnostic census runner changed row order or identity")
        validate_profile_derivation_record({
            "profile": entry.get("selected_profile"), "immediate_witness": entry.get("immediate_witness"),
        }, f"diagnostic census profile derivation {row.get('id')}")
        selected_profile_counts[entry["selected_profile"]["name"]] += 1
        entries.append(entry)
        validation = entry["validation"]
        stdout_bytes = validation["stdout_bytes"]
        stderr_bytes = validation["stderr_bytes"]
        base.require(
            isinstance(stdout_bytes, int) and not isinstance(stdout_bytes, bool) and 0 <= stdout_bytes <= census["maximum_stream_bytes"]
            and isinstance(stderr_bytes, int) and not isinstance(stderr_bytes, bool) and 0 <= stderr_bytes <= census["maximum_stream_bytes"],
            f"diagnostic census stream cap changed: {row.get('id')}",
        )
        base.require(stdout_bytes + stderr_bytes <= census["maximum_row_output_bytes"], f"diagnostic census row cap changed: {row.get('id')}")
        total_output_bytes += stdout_bytes + stderr_bytes
        key = "timeout" if validation["exit_code"] is None else str(validation["exit_code"])
        exit_counts[key] = exit_counts.get(key, 0) + 1
    base.require(total_output_bytes <= census["maximum_total_output_bytes"], "diagnostic census total output cap changed")
    base.require(selected_profile_counts == policy["profile_derivation"]["expected_corpus_profile_counts"], "diagnostic census profile counts changed")
    return entries, {
        "rows": len(entries),
        "total_process_output_bytes": total_output_bytes,
        "exit_code_counts": {key: exit_counts[key] for key in sorted(exit_counts)},
        "selected_profile_counts": selected_profile_counts,
    }


def build_diagnostic_census(args: argparse.Namespace) -> dict:
    policy = load_policy()
    base.require(resource is not None, "diagnostic census requires POSIX RLIMIT_FSIZE support")
    reference_receipt = verify_reference_inputs(args, policy)
    validator_build_dir = Path(args.wgpu_validator_build_dir).resolve()
    validator_receipt, validator_binary, validator_identity = verify_validator_build(validator_build_dir, policy)
    corpus = qualify_root(Path(args.reference_artifact_dir), "accepted reference corpus")
    validator_root = qualify_root(validator_build_dir, "qualified wgpu validator build")
    with tempfile.TemporaryDirectory(prefix="fn64-wgpu-diagnostic-census-") as temporary:
        private_root = Path(temporary).resolve(strict=True)
        staging_rows = qualify_private_root(private_root, [corpus, validator_root])
        validator = stage_validator(validator_binary, private_root, policy)

        def run_row(row: dict) -> dict:
            verify_qualified_root(corpus, "accepted reference corpus")
            reference.verify_identity_rows(staging_rows, "wgpu diagnostic census staging")
            return diagnose_row(validator, corpus.path, private_root, row, policy)

        entries, totals = collect_diagnostic_rows(
            reference_receipt["entries"],
            run_row,
            policy,
        )
        verify_qualified_root(corpus, "accepted reference corpus")
        verify_qualified_root(validator_root, "qualified wgpu validator build")
        reference.verify_identity_rows(staging_rows, "wgpu diagnostic census staging")
    census_policy = policy["diagnostic_census"]
    result = {
        "schema": census_policy["schema"],
        "authority": census_policy["authority"],
        "reference_corpus": {
            "receipt_sha256": reference_receipt["receipt_sha256"],
            "receipt_file_sha256": policy["reference_corpus"]["receipt_file_sha256"],
            "artifact_set_sha256": reference_receipt["artifact_set_sha256"],
            "entry_order_sha256": policy["reference_corpus"]["entry_order_sha256"],
            "row_count": len(entries),
        },
        "wgpu_validator": {
            "build_receipt_sha256": validator_receipt["receipt_sha256"],
            "binary_sha256": validator_receipt["binary_sha256"],
            "identity": validator_identity["identity"],
        },
        "entries": entries,
        "totals": totals,
        "runtime_ready": False,
        "claim_boundary": census_policy["claim_boundary"],
    }
    encoded = base.pretty_json(result)
    base.require(len(encoded) <= census_policy["maximum_census_bytes"], "diagnostic census JSON exceeds its output cap")
    base.require(not base.LOCAL_PATH_RE.search(json.dumps(result)), "diagnostic census leaked a machine-local path")
    return result


def runtime_readiness(entries: list[dict], policy: dict) -> dict:
    reasons = []
    if any(row["outcome"] == "blocked-known" for row in entries):
        reasons.append("blocked-known-ingestion-row")
    reasons.extend(policy["runtime_readiness"]["reason_order"][1:])
    base.require(reasons == [reason for reason in policy["runtime_readiness"]["reason_order"] if reason != "blocked-known-ingestion-row" or any(row["outcome"] == "blocked-known" for row in entries)], "runtime readiness reason order changed")
    return {"runtime_ready": False, "reasons": reasons}


def build_assessment(args: argparse.Namespace) -> dict:
    policy = load_policy()
    reference_receipt = verify_reference_inputs(args, policy)
    validator_build_dir = Path(args.wgpu_validator_build_dir).resolve()
    validator_receipt, validator_binary, validator_identity = verify_validator_build(validator_build_dir, policy)
    corpus = qualify_root(Path(args.reference_artifact_dir), "accepted reference corpus")
    validator_root = qualify_root(validator_build_dir, "qualified wgpu validator build")
    with tempfile.TemporaryDirectory(prefix="fn64-wgpu-assessment-") as temporary:
        private_root = Path(temporary).resolve(strict=True)
        staging_rows = qualify_private_root(private_root, [corpus, validator_root])
        validator = stage_validator(validator_binary, private_root, policy)
        entries = []
        seen = set()
        for reference_row in reference_receipt["entries"]:
            base.require(reference_row.get("id") not in seen, "reference assessment row id repeated")
            seen.add(reference_row.get("id"))
            verify_qualified_root(corpus, "accepted reference corpus")
            reference.verify_identity_rows(staging_rows, "wgpu assessment staging")
            entries.append(assess_row(validator, corpus.path, private_root, reference_row, policy))
        verify_qualified_root(corpus, "accepted reference corpus")
        verify_qualified_root(validator_root, "qualified wgpu validator build")
    counts = {name: sum(row["outcome"] == name for row in entries) for name in policy["outcomes"]["order"]}
    base.require(sum(counts.values()) == policy["reference_corpus"]["row_count"], "wgpu assessment outcome denominator is incomplete")
    profile_counts = {name: sum(row["selected_profile"]["name"] == name for row in entries) for name in PROFILE_NAMES}
    base.require(profile_counts == policy["profile_derivation"]["expected_corpus_profile_counts"], "wgpu assessment profile counts changed")
    receipt = base.add_receipt_hash({
        "schema": policy["receipt_schema"], "status": "complete",
        "producer_sha256": base.digest_file(TOOL_PATH), "policy_sha256": base.digest_file(POLICY_PATH),
        "reference_corpus": {
            "receipt_sha256": reference_receipt["receipt_sha256"],
            "receipt_file_sha256": policy["reference_corpus"]["receipt_file_sha256"],
            "artifact_set_sha256": reference_receipt["artifact_set_sha256"],
            "denominator_sha256": reference_receipt["denominator_sha256"],
            "source_snapshot_set_sha256": reference_receipt["source_snapshot"]["source_set_sha256"],
            "orchestration_producer_sha256": reference_receipt["orchestration_producer_sha256"],
            "artifact_producer_sha256": reference_receipt["artifact_producer_sha256"],
            "reference_policy_sha256": reference_receipt["reference_policy_sha256"],
            "artifact_policy_sha256": reference_receipt["artifact_policy_sha256"],
            "dxc_build_receipt_sha256": reference_receipt["dxc_build_receipt_sha256"],
            "dxc_compiler_sha256": reference_receipt["dxc_compiler_sha256"],
            "spirv_val_build_receipt_sha256": reference_receipt["spirv_val_build_receipt_sha256"],
            "spirv_grammar_sha256": reference_receipt["spirv_grammar"]["sha256"],
            "entry_order_sha256": policy["reference_corpus"]["entry_order_sha256"],
            "row_count": len(entries), "file_count": policy["reference_corpus"]["file_count"],
        },
        "wgpu_validator": {
            "build_receipt_sha256": validator_receipt["receipt_sha256"],
            "binary_sha256": validator_receipt["binary_sha256"],
            "source_set_sha256": validator_receipt["source_set_sha256"],
            "cargo_lock_sha256": validator_receipt["cargo_lock_sha256"],
            "dependency_set_sha256": validator_receipt["dependency_set_sha256"],
            "identity": validator_identity["identity"],
        },
        "assessment_contract": {
            "strict_capabilities": True, "noop_checked_shader_module": True,
            "arguments": policy["wgpu_validator"]["arguments"],
            "profiles": policy["wgpu_validator"]["identity"]["profiles"],
            "profile_derivation": policy["profile_derivation"],
            "controlled_environment": policy["wgpu_validator"]["controlled_environment"],
            "outcome_order": policy["outcomes"]["order"],
        },
        "entries": entries, "outcome_counts": counts, "profile_counts": profile_counts,
        "assessment_set_sha256": base.digest_bytes(base.canonical_json(entries)),
        "runtime_readiness": runtime_readiness(entries, policy),
        "claim_boundary": policy["claim_boundary"],
    })
    encoded = base.pretty_json(receipt)
    base.require(len(encoded) <= policy["limits"]["maximum_receipt_bytes"], "wgpu assessment receipt exceeds policy limit")
    base.require(not base.LOCAL_PATH_RE.search(json.dumps(receipt)), "wgpu assessment receipt leaked a machine-local path")
    return receipt


def write_assessment(args: argparse.Namespace) -> dict:
    receipt = build_assessment(args)
    output = Path(args.output_dir).resolve()
    base.prepare_output_directory(output)
    base.write_new_private_file(output / RECEIPT_NAME, base.pretty_json(receipt))
    return receipt


def validate_assessment_receipt(receipt: object, policy: dict) -> None:
    base.require_keys(receipt, {
        "schema", "status", "producer_sha256", "policy_sha256", "reference_corpus",
        "wgpu_validator", "assessment_contract", "entries", "outcome_counts",
        "profile_counts", "assessment_set_sha256", "runtime_readiness", "claim_boundary", "receipt_sha256",
    }, "wgpu assessment receipt")
    base.require(receipt["schema"] == policy["receipt_schema"] and receipt["status"] == "complete", "wgpu assessment receipt is incomplete")
    base.validate_receipt_hash(receipt)
    base.require(receipt["producer_sha256"] == base.digest_file(TOOL_PATH), "wgpu assessment producer changed")
    base.require(receipt["policy_sha256"] == base.digest_file(POLICY_PATH), "wgpu assessment policy changed")
    base.require(receipt["claim_boundary"] == policy["claim_boundary"], "wgpu assessment claim boundary changed")
    base.require(receipt["reference_corpus"] == {
        "receipt_sha256": policy["reference_corpus"]["receipt_sha256"],
        "receipt_file_sha256": policy["reference_corpus"]["receipt_file_sha256"],
        "artifact_set_sha256": policy["reference_corpus"]["artifact_set_sha256"],
        "denominator_sha256": policy["reference_corpus"]["denominator_sha256"],
        "source_snapshot_set_sha256": policy["reference_corpus"]["source_snapshot_set_sha256"],
        "orchestration_producer_sha256": policy["reference_corpus"]["orchestration_producer_sha256"],
        "artifact_producer_sha256": policy["reference_corpus"]["artifact_producer_sha256"],
        "reference_policy_sha256": policy["reference_corpus"]["reference_policy_sha256"],
        "artifact_policy_sha256": policy["reference_corpus"]["artifact_policy_sha256"],
        "dxc_build_receipt_sha256": policy["reference_corpus"]["dxc_build_receipt_sha256"],
        "dxc_compiler_sha256": policy["reference_corpus"]["dxc_compiler_sha256"],
        "spirv_val_build_receipt_sha256": policy["reference_corpus"]["spirv_val_build_receipt_sha256"],
        "spirv_grammar_sha256": policy["reference_corpus"]["spirv_grammar_sha256"],
        "entry_order_sha256": policy["reference_corpus"]["entry_order_sha256"],
        "row_count": policy["reference_corpus"]["row_count"],
        "file_count": policy["reference_corpus"]["file_count"],
    }, "wgpu assessment reference identity changed")
    expected_validator = policy["wgpu_validator"]
    base.require(receipt["wgpu_validator"] == {
        "build_receipt_sha256": expected_validator["build_receipt_sha256"], "binary_sha256": expected_validator["binary_sha256"],
        "source_set_sha256": expected_validator["source_set_sha256"], "cargo_lock_sha256": expected_validator["cargo_lock_sha256"],
        "dependency_set_sha256": expected_validator["dependency_set_sha256"], "identity": expected_validator["identity"],
    }, "wgpu assessment validator identity changed")
    base.require(receipt["assessment_contract"] == {
        "strict_capabilities": True, "noop_checked_shader_module": True,
        "arguments": expected_validator["arguments"], "profiles": expected_validator["identity"]["profiles"],
        "profile_derivation": policy["profile_derivation"],
        "controlled_environment": expected_validator["controlled_environment"],
        "outcome_order": policy["outcomes"]["order"],
    }, "wgpu assessment contract changed")
    entries = receipt["entries"]
    base.require(isinstance(entries, list) and len(entries) == policy["reference_corpus"]["row_count"], "wgpu assessment row denominator changed")
    ids = []
    for index, row in enumerate(entries):
        base.require_keys(row, {
            "id", "source", "stage", "entry", "spirv_artifact", "spirv_sha256", "spirv_bytes",
            "semantic_inventory", "immediate_witness", "selected_profile",
            "scalar_layout_witness", "sampled_buffer_witness", "fragment_interface_witness",
            "outcome", "reason_code", "validation", "validation_record",
        }, f"wgpu assessment row {index}")
        base.require(isinstance(row["id"], str) and row["id"] and row["id"] not in ids, "wgpu assessment row id repeated")
        ids.append(row["id"])
        safe_relative(row["spirv_artifact"], f"wgpu assessment artifact {index}")
        require_sha(row["spirv_sha256"], f"wgpu assessment artifact digest {index}")
        base.require(isinstance(row["spirv_bytes"], int) and row["spirv_bytes"] > 0, "wgpu assessment artifact length is invalid")
        validate_inventory_record(row["semantic_inventory"], f"wgpu assessment inventory {index}")
        validate_profile_derivation_record({
            "profile": row["selected_profile"], "immediate_witness": row["immediate_witness"],
        }, f"wgpu assessment profile derivation {index}")
        base.require(row["outcome"] in policy["outcomes"]["order"], "wgpu assessment row outcome changed")
        base.require_keys(row["validation"], {"arguments", "exit_code", "stdout_sha256", "stdout_bytes", "stderr_sha256", "stderr_bytes"}, f"wgpu assessment transcript {index}")
        require_sha(row["validation"]["stdout_sha256"], f"wgpu assessment stdout {index}")
        require_sha(row["validation"]["stderr_sha256"], f"wgpu assessment stderr {index}")
        expected_args = validator_arguments(row["selected_profile"]["name"], row["stage"], row["entry"])
        base.require(row["validation"]["arguments"] == expected_args, "wgpu assessment row argv changed")
        witness_fields = ("scalar_layout_witness", "sampled_buffer_witness", "fragment_interface_witness")
        if row["outcome"] == "blocked-known":
            base.require(row["validation_record"] is None, "blocked-known row has a success record")
            shader = policy["outcomes"]["blocked_known_shader_nonuniform"]
            scalar = policy["outcomes"]["blocked_known_scalar_layout"]
            sampled_buffer = policy["outcomes"]["blocked_known_sampled_buffer"]
            fragment_interface = policy["outcomes"]["blocked_known_fragment_direct_blend_src_index_output"]
            matching_witnesses = [name for name in witness_fields if row[name] is not None]
            base.require(len(matching_witnesses) <= 1, f"blocked-known row has more than one matching witness: {row['id']}")
            if row["reason_code"] == shader["reason_code"]:
                base.require(row["validation"]["exit_code"] == shader["exit_code"] and row["validation"]["stdout_sha256"] == shader["stdout_sha256"] and row["validation"]["stderr_sha256"] == shader["stderr_sha256"], "blocked-known ShaderNonUniform row transcript changed")
                base.require(row["validation"]["stdout_bytes"] == 0 and row["validation"]["stderr_bytes"] == len(KNOWN_STDERR), "blocked-known ShaderNonUniform row output lengths changed")
                inventory_supports_known_blocker(row["semantic_inventory"], policy)
                base.require(matching_witnesses in ([], ["scalar_layout_witness"]), f"blocked-known ShaderNonUniform row witness mismatch: {row['id']}")
                if row["scalar_layout_witness"] is not None:
                    validate_scalar_witness_record(row["scalar_layout_witness"], policy, f"wgpu assessment scalar witness {index}")
            elif row["reason_code"] == scalar["reason_code"]:
                base.require(row["validation"]["exit_code"] == scalar["exit_code"] and row["validation"]["stdout_sha256"] == scalar["stdout_sha256"] and row["validation"]["stderr_sha256"] == scalar["stderr_sha256"], "blocked-known scalar-layout row transcript changed")
                base.require(row["validation"]["stdout_bytes"] == 0 and row["validation"]["stderr_bytes"] == len(SCALAR_LAYOUT_STDERR), "blocked-known scalar-layout row output lengths changed")
                base.require(matching_witnesses == ["scalar_layout_witness"], f"blocked-known scalar-layout row lacks its exact witness: {row['id']}")
                validate_scalar_witness_record(row["scalar_layout_witness"], policy, f"wgpu assessment scalar witness {index}")
            elif row["reason_code"] == sampled_buffer["reason_code"]:
                base.require(row["validation"]["exit_code"] == sampled_buffer["exit_code"] and row["validation"]["stdout_sha256"] == sampled_buffer["stdout_sha256"] and row["validation"]["stderr_sha256"] == sampled_buffer["stderr_sha256"], "blocked-known sampled-buffer row transcript changed")
                base.require(row["validation"]["stdout_bytes"] == 0 and row["validation"]["stderr_bytes"] == len(SAMPLED_BUFFER_STDERR), "blocked-known sampled-buffer row output lengths changed")
                base.require(matching_witnesses == ["sampled_buffer_witness"], f"blocked-known sampled-buffer row lacks its exact witness: {row['id']}")
                validate_sampled_buffer_witness_record(row["sampled_buffer_witness"], policy, f"wgpu assessment sampled-buffer witness {index}")
            elif row["reason_code"] == fragment_interface["reason_code"]:
                base.require(row["validation"]["exit_code"] == fragment_interface["exit_code"] and row["validation"]["stdout_sha256"] == fragment_interface["stdout_sha256"] and row["validation"]["stderr_sha256"] == fragment_interface["stderr_sha256"], "blocked-known fragment-interface row transcript changed")
                base.require(row["validation"]["stdout_bytes"] == 0 and row["validation"]["stderr_bytes"] == len(FRAGMENT_INTERFACE_STDERR), "blocked-known fragment-interface row output lengths changed")
                base.require(matching_witnesses == ["fragment_interface_witness"], f"blocked-known fragment-interface row lacks its exact witness: {row['id']}")
                validate_fragment_interface_witness_record(row["fragment_interface_witness"], policy, f"wgpu assessment fragment-interface witness {index}")
            else:
                raise base.ArtifactError("blocked-known row reason changed")
        else:
            base.require(
                row["reason_code"] is None and all(row[name] is None for name in witness_fields) and isinstance(row["validation_record"], dict),
                "ingestible row record changed",
            )
            base.require(row["validation"]["exit_code"] == 0 and row["validation"]["stderr_sha256"] == EMPTY_SHA256, "ingestible row transcript changed")
            base.require(row["validation"]["stderr_bytes"] == 0 and row["validation"]["stdout_bytes"] > 0, "ingestible row output lengths changed")
            expected_record = validator_success_record(
                row["selected_profile"]["name"], row["stage"], row["entry"], row["spirv_bytes"]
            )
            base.require(row["validation_record"] == expected_record, "ingestible row result changed")
            expected_stdout = validator_success_bytes(
                row["selected_profile"]["name"], row["stage"], row["entry"], row["spirv_bytes"]
            )
            base.require(row["validation"]["stdout_sha256"] == base.digest_bytes(expected_stdout) and row["validation"]["stdout_bytes"] == len(expected_stdout), "ingestible row stdout bytes changed")
    base.require(base.digest_bytes(base.canonical_json(ids)) == policy["reference_corpus"]["entry_order_sha256"], "wgpu assessment row order changed")
    counts = {name: sum(row["outcome"] == name for row in entries) for name in policy["outcomes"]["order"]}
    base.require(receipt["outcome_counts"] == counts and sum(counts.values()) == len(entries), "wgpu assessment outcome counts changed")
    profile_counts = {name: sum(row["selected_profile"]["name"] == name for row in entries) for name in PROFILE_NAMES}
    base.require(receipt["profile_counts"] == profile_counts == policy["profile_derivation"]["expected_corpus_profile_counts"], "wgpu assessment profile counts changed")
    base.require(receipt["assessment_set_sha256"] == base.digest_bytes(base.canonical_json(entries)), "wgpu assessment set identity changed")
    base.require(receipt["runtime_readiness"] == runtime_readiness(entries, policy), "wgpu assessment runtime readiness changed")
    base.require(not receipt["runtime_readiness"]["runtime_ready"], "M2.5b cannot assert runtime readiness")


def verify_assessment(args: argparse.Namespace) -> dict:
    policy = load_policy()
    assessment_dir = Path(args.assessment_dir).resolve()
    receipt = base.load_canonical_json(assessment_dir / RECEIPT_NAME, policy["limits"]["maximum_receipt_bytes"], "wgpu assessment receipt")
    validate_assessment_receipt(receipt, policy)
    expected = build_assessment(args)
    base.require(receipt == expected, "wgpu assessment receipt differs from fresh strict assessment")
    actual = {path.relative_to(assessment_dir).as_posix() for path in assessment_dir.rglob("*") if path.is_file() or path.is_symlink()}
    base.require(actual == {RECEIPT_NAME}, f"wgpu assessment output denominator changed: {sorted(actual)}")
    return receipt


def common_arguments(command: argparse.ArgumentParser) -> None:
    command.add_argument("--reference-artifact-dir", required=True)
    command.add_argument("--wgpu-validator-build-dir", required=True)


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    sub = result.add_subparsers(dest="command", required=True)
    assess = sub.add_parser("assess")
    common_arguments(assess)
    assess.add_argument("--output-dir", required=True)
    verify = sub.add_parser("verify")
    common_arguments(verify)
    verify.add_argument("--assessment-dir", required=True)
    ready = sub.add_parser("runtime-ready")
    common_arguments(ready)
    ready.add_argument("--assessment-dir", required=True)
    census = sub.add_parser("diagnostic-census")
    common_arguments(census)
    sub.add_parser("selftest")
    return result


def selftest() -> None:
    policy = load_policy()
    base.require(base.digest_bytes(KNOWN_STDERR) == policy["outcomes"]["blocked_known_shader_nonuniform"]["stderr_sha256"], "known ShaderNonUniform blocker transcript drift")
    base.require(base.digest_bytes(SCALAR_LAYOUT_STDERR) == policy["outcomes"]["blocked_known_scalar_layout"]["stderr_sha256"], "known scalar-layout blocker transcript drift")
    base.require(base.digest_bytes(SAMPLED_BUFFER_STDERR) == policy["outcomes"]["blocked_known_sampled_buffer"]["stderr_sha256"], "known sampled-buffer blocker transcript drift")
    base.require(base.digest_bytes(FRAGMENT_INTERFACE_STDERR) == policy["outcomes"]["blocked_known_fragment_direct_blend_src_index_output"]["stderr_sha256"], "known fragment-interface blocker transcript drift")
    base.require(runtime_readiness([], policy)["runtime_ready"] is False, "M2.5b runtime readiness changed")
    base.require(policy["diagnostic_census"]["authority"] == "non-authoritative-diagnostic-only", "diagnostic census authority changed")


def main() -> int:
    args = parser().parse_args()
    try:
        if args.command == "selftest":
            selftest()
            print("rt64 wgpu shader assessment self-test passed")
        elif args.command == "assess":
            receipt = write_assessment(args)
            print(f"wgpu shader assessment receipt: {receipt['receipt_sha256']}")
        elif args.command == "verify":
            receipt = verify_assessment(args)
            print(f"wgpu shader assessment verified: {receipt['receipt_sha256']}")
        elif args.command == "runtime-ready":
            receipt = verify_assessment(args)
            print(base.pretty_json(receipt["runtime_readiness"]).decode(), end="")
            return RUNTIME_NOT_READY_EXIT
        elif args.command == "diagnostic-census":
            census = build_diagnostic_census(args)
            print(base.pretty_json(census).decode(), end="")
        else:
            raise base.ArtifactError(f"unknown command {args.command}")
    except (base.ArtifactError, OSError, subprocess.SubprocessError) as error:
        print(f"rt64-wgpu-shader-assessment: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
