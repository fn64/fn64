#!/usr/bin/env python3
"""Assess the accepted RT64 reference SPIR-V corpus through strict wgpu 30."""

from __future__ import annotations

import argparse
import copy
import json
import os
import stat
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rt64_reference_shader_artifacts as reference
import rt64_shader_artifacts as base


ROOT = Path(__file__).resolve().parents[1]
TOOL_PATH = Path(__file__).resolve()
POLICY_PATH = ROOT / "docs/rt64-wgpu-shader-assessment-schema.json"
RECEIPT_NAME = "assessment-receipt.json"
EMPTY_SHA256 = base.digest_bytes(b"")
KNOWN_STDERR = b"fn64-wgpu-shader-validator: wgpu 30 SPIR-V parse failed: unsupported capability ShaderNonUniform\n"
RUNTIME_NOT_READY_EXIT = 78


def safe_relative(value: object, label: str) -> PurePosixPath:
    base.require(isinstance(value, str), f"{label} is not text")
    path = PurePosixPath(value)
    base.require(path.parts and not path.is_absolute() and ".." not in path.parts, f"unsafe {label}")
    base.require(path.as_posix() == value, f"non-canonical {label}")
    return path


def require_sha(value: object, label: str) -> str:
    base.require(isinstance(value, str) and base.SHA256_RE.fullmatch(value) is not None, f"{label} is not a SHA-256 digest")
    return value


def load_policy() -> dict:
    policy = base.load_json(POLICY_PATH)
    base.require_keys(policy, {
        "schema", "direct_consumers", "receipt_schema", "receipt_path", "producer", "reference_corpus",
        "wgpu_validator", "outcomes", "runtime_readiness", "limits", "claim_boundary",
    }, "wgpu assessment policy")
    base.require(policy["schema"] == "fn64.rt64-wgpu-shader-assessment-policy.v1", "unsupported wgpu assessment policy")
    base.require(policy["direct_consumers"] == [
        "tools/rt64_wgpu_shader_assessment.py",
        "tools/test_rt64_wgpu_shader_assessment.py",
    ], "wgpu assessment policy consumer denominator changed")
    base.require(policy["receipt_schema"] == "fn64.rt64-wgpu-shader-assessment.v1", "unsupported wgpu assessment receipt schema")
    base.require(policy["receipt_path"] == RECEIPT_NAME, "wgpu assessment receipt path changed")
    base.require_keys(policy["producer"], {"path", "sha256"}, "wgpu assessment producer")
    base.require(policy["producer"]["path"] == "tools/rt64_wgpu_shader_assessment.py", "wgpu assessment producer path changed")
    base.require(require_sha(policy["producer"]["sha256"], "wgpu assessment producer digest") == base.digest_file(TOOL_PATH), "wgpu assessment producer digest changed")
    base.require_keys(policy["reference_corpus"], {
        "receipt_schema", "receipt_sha256", "receipt_file_sha256", "artifact_set_sha256",
        "denominator_sha256", "source_snapshot_set_sha256", "orchestration_producer_sha256",
        "artifact_producer_sha256", "reference_policy_sha256", "artifact_policy_sha256",
        "dxc_build_receipt_sha256", "dxc_compiler_sha256", "spirv_val_build_receipt_sha256",
        "spirv_grammar_sha256", "entry_order_sha256", "row_count",
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
    validator = policy["wgpu_validator"]
    base.require_keys(validator, {
        "build_receipt_schema", "build_receipt_sha256", "binary_sha256", "source_set_sha256",
        "cargo_lock_sha256", "dependency_set_sha256", "identity", "arguments", "controlled_environment",
    }, "wgpu validator policy")
    for key in ("build_receipt_sha256", "binary_sha256", "source_set_sha256", "cargo_lock_sha256", "dependency_set_sha256"):
        require_sha(validator[key], f"wgpu validator {key}")
    base.require(validator["build_receipt_schema"] == "fn64.wgpu-shader-validator-build.v1", "wgpu validator build schema changed")
    base.require_keys(validator["identity"], {"schema", "wgpu_major", "wgpu_version", "naga_version", "backend", "validation"}, "wgpu validator identity policy")
    base.require(validator["identity"] == {
        "schema": "fn64.wgpu-shader-validator.v1", "wgpu_major": 30,
        "wgpu_version": "30.0.0", "naga_version": "30.0.0", "backend": "noop",
        "validation": "wgpu-30-baseline-naga-validation-plus-checked-api",
    }, "wgpu validator identity denominator changed")
    base.require(validator["arguments"] == ["--shader", "<private-staged-spv>", "--stage", "<stage>", "--entry", "<entry>"], "wgpu validator argv changed")
    base.require(validator["controlled_environment"] == {
        "HOME": "<private-staging-root>", "LANG": "C", "LC_ALL": "C", "PATH": "/usr/bin:/bin",
    }, "wgpu validator environment changed")
    outcomes = policy["outcomes"]
    base.require_keys(outcomes, {"order", "ingestible", "blocked_known"}, "wgpu outcome policy")
    base.require(outcomes["order"] == ["ingestible", "blocked-known"], "wgpu outcome order changed")
    base.require_keys(outcomes["ingestible"], {"exit_code", "stdout_schema", "stdout_status", "stderr_sha256"}, "ingestible outcome")
    base.require(outcomes["ingestible"] == {
        "exit_code": 0, "stdout_schema": "fn64.wgpu-shader-validation.v1",
        "stdout_status": "passed", "stderr_sha256": EMPTY_SHA256,
    }, "ingestible outcome changed")
    blocked = outcomes["blocked_known"]
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
    reference.verify(argparse.Namespace(
        port_dir=args.port_dir, oracle_dir=args.oracle_dir, dxc_dir=args.dxc_dir,
        dxc_build_dir=args.dxc_build_dir, spirv_val_build_dir=args.spirv_val_build_dir,
        artifact_dir=args.reference_artifact_dir,
    ))
    corpus_dir = Path(args.reference_artifact_dir).resolve()
    receipt_path = corpus_dir / reference.RECEIPT_PATH
    receipt_bytes, info = base.stable_regular_bytes(receipt_path, policy["limits"]["maximum_receipt_bytes"], "accepted reference receipt")
    base.require(info.st_nlink == 1, "accepted reference receipt has another hardlink")
    base.require(base.digest_bytes(receipt_bytes) == policy["reference_corpus"]["receipt_file_sha256"], "accepted reference receipt file identity changed")
    try:
        receipt = json.loads(receipt_bytes)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise base.ArtifactError(f"accepted reference receipt is malformed: {error}") from error
    base.require(base.canonical_json(receipt) == receipt_bytes, "accepted reference receipt is not canonical")
    expected = policy["reference_corpus"]
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
    return receipt


def verify_validator_build(build_dir: Path, policy: dict) -> tuple[dict, Path, dict]:
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


def inventory_supports_known_blocker(inventory: object, policy: dict) -> None:
    base.require(isinstance(inventory, dict), "blocked-known row has no semantic inventory")
    blocked = policy["outcomes"]["blocked_known"]
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


def classify_result(result: subprocess.CompletedProcess[bytes], row: dict, artifact_bytes: bytes, policy: dict) -> tuple[str, str | None, dict | None]:
    stdout_sha = base.digest_bytes(result.stdout)
    stderr_sha = base.digest_bytes(result.stderr)
    inventory = row.get("semantic_inventory")
    has_shader_nonuniform = isinstance(inventory, dict) and policy["outcomes"]["blocked_known"]["required_capability"] in [
        {"name": item.get("name"), "value": item.get("value")}
        for item in inventory.get("capabilities", []) if isinstance(item, dict)
    ]
    ingestible = policy["outcomes"]["ingestible"]
    if result.returncode == ingestible["exit_code"] and stderr_sha == ingestible["stderr_sha256"]:
        base.require(not has_shader_nonuniform, f"ShaderNonUniform witness unexpectedly passed strict wgpu ingestion: {row['id']}")
        try:
            record = json.loads(result.stdout)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise base.ArtifactError(f"ingestible row emitted malformed JSON: {row['id']}: {error}") from error
        base.require_keys(record, {"schema", "status", "wgpu_major", "stage", "entry", "module_bytes"}, f"ingestible stdout {row['id']}")
        expected_record = {
            "schema": ingestible["stdout_schema"], "status": ingestible["stdout_status"],
            "wgpu_major": 30, "stage": row["stage"], "entry": row["entry"],
            "module_bytes": len(artifact_bytes),
        }
        base.require(record == expected_record, f"ingestible stdout changed: {row['id']}")
        expected_bytes = (
            f'{{"schema":"{ingestible["stdout_schema"]}","status":"{ingestible["stdout_status"]}",'
            f'"wgpu_major":30,"stage":"{row["stage"]}","entry":"{row["entry"]}",'
            f'"module_bytes":{len(artifact_bytes)}}}\n'
        ).encode()
        base.require(result.stdout == expected_bytes, f"ingestible stdout bytes changed: {row['id']}")
        return "ingestible", None, record
    blocked = policy["outcomes"]["blocked_known"]
    if result.returncode == blocked["exit_code"] and stdout_sha == blocked["stdout_sha256"] and stderr_sha == blocked["stderr_sha256"]:
        base.require(result.stderr == KNOWN_STDERR, f"blocked-known stderr bytes changed: {row['id']}")
        inventory_supports_known_blocker(row.get("semantic_inventory"), policy)
        return "blocked-known", blocked["reason_code"], None
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
    arguments = [str(validator), "--shader", str(snapshot), "--stage", row["stage"], "--entry", row["entry"]]
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
    outcome, reason, stdout_record = classify_result(result, row, artifact_bytes, policy)
    transcript = {
        "arguments": ["--shader", "<private-staged-spv>", "--stage", row["stage"], "--entry", row["entry"]],
        "exit_code": result.returncode, "stdout_sha256": base.digest_bytes(result.stdout),
        "stdout_bytes": len(result.stdout), "stderr_sha256": base.digest_bytes(result.stderr),
        "stderr_bytes": len(result.stderr),
    }
    return {
        "id": row["id"], "source": row["source"], "stage": row["stage"], "entry": row["entry"],
        "spirv_artifact": row["spirv_artifact"], "spirv_sha256": digest, "spirv_bytes": len(artifact_bytes),
        "semantic_inventory": copy.deepcopy(row["semantic_inventory"]), "outcome": outcome,
        "reason_code": reason, "validation": transcript, "validation_record": stdout_record,
    }


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
            "row_count": len(entries),
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
            "controlled_environment": policy["wgpu_validator"]["controlled_environment"],
            "outcome_order": policy["outcomes"]["order"],
        },
        "entries": entries, "outcome_counts": counts,
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
        "assessment_set_sha256", "runtime_readiness", "claim_boundary", "receipt_sha256",
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
    }, "wgpu assessment reference identity changed")
    expected_validator = policy["wgpu_validator"]
    base.require(receipt["wgpu_validator"] == {
        "build_receipt_sha256": expected_validator["build_receipt_sha256"], "binary_sha256": expected_validator["binary_sha256"],
        "source_set_sha256": expected_validator["source_set_sha256"], "cargo_lock_sha256": expected_validator["cargo_lock_sha256"],
        "dependency_set_sha256": expected_validator["dependency_set_sha256"], "identity": expected_validator["identity"],
    }, "wgpu assessment validator identity changed")
    base.require(receipt["assessment_contract"] == {
        "strict_capabilities": True, "noop_checked_shader_module": True,
        "arguments": expected_validator["arguments"], "controlled_environment": expected_validator["controlled_environment"],
        "outcome_order": policy["outcomes"]["order"],
    }, "wgpu assessment contract changed")
    entries = receipt["entries"]
    base.require(isinstance(entries, list) and len(entries) == policy["reference_corpus"]["row_count"], "wgpu assessment row denominator changed")
    ids = []
    for index, row in enumerate(entries):
        base.require_keys(row, {
            "id", "source", "stage", "entry", "spirv_artifact", "spirv_sha256", "spirv_bytes",
            "semantic_inventory", "outcome", "reason_code", "validation", "validation_record",
        }, f"wgpu assessment row {index}")
        base.require(isinstance(row["id"], str) and row["id"] and row["id"] not in ids, "wgpu assessment row id repeated")
        ids.append(row["id"])
        safe_relative(row["spirv_artifact"], f"wgpu assessment artifact {index}")
        require_sha(row["spirv_sha256"], f"wgpu assessment artifact digest {index}")
        base.require(isinstance(row["spirv_bytes"], int) and row["spirv_bytes"] > 0, "wgpu assessment artifact length is invalid")
        validate_inventory_record(row["semantic_inventory"], f"wgpu assessment inventory {index}")
        base.require(row["outcome"] in policy["outcomes"]["order"], "wgpu assessment row outcome changed")
        base.require_keys(row["validation"], {"arguments", "exit_code", "stdout_sha256", "stdout_bytes", "stderr_sha256", "stderr_bytes"}, f"wgpu assessment transcript {index}")
        require_sha(row["validation"]["stdout_sha256"], f"wgpu assessment stdout {index}")
        require_sha(row["validation"]["stderr_sha256"], f"wgpu assessment stderr {index}")
        expected_args = ["--shader", "<private-staged-spv>", "--stage", row["stage"], "--entry", row["entry"]]
        base.require(row["validation"]["arguments"] == expected_args, "wgpu assessment row argv changed")
        if row["outcome"] == "blocked-known":
            blocked = policy["outcomes"]["blocked_known"]
            base.require(row["reason_code"] == blocked["reason_code"] and row["validation_record"] is None, "blocked-known row reason changed")
            base.require(row["validation"]["exit_code"] == blocked["exit_code"] and row["validation"]["stdout_sha256"] == blocked["stdout_sha256"] and row["validation"]["stderr_sha256"] == blocked["stderr_sha256"], "blocked-known row transcript changed")
            base.require(row["validation"]["stdout_bytes"] == 0 and row["validation"]["stderr_bytes"] == len(KNOWN_STDERR), "blocked-known row output lengths changed")
            inventory_supports_known_blocker(row["semantic_inventory"], policy)
        else:
            base.require(row["reason_code"] is None and isinstance(row["validation_record"], dict), "ingestible row record changed")
            base.require(row["validation"]["exit_code"] == 0 and row["validation"]["stderr_sha256"] == EMPTY_SHA256, "ingestible row transcript changed")
            base.require(row["validation"]["stderr_bytes"] == 0 and row["validation"]["stdout_bytes"] > 0, "ingestible row output lengths changed")
            expected_record = {
                "schema": policy["outcomes"]["ingestible"]["stdout_schema"],
                "status": policy["outcomes"]["ingestible"]["stdout_status"],
                "wgpu_major": 30, "stage": row["stage"], "entry": row["entry"],
                "module_bytes": row["spirv_bytes"],
            }
            base.require(row["validation_record"] == expected_record, "ingestible row result changed")
            expected_stdout = (
                f'{{"schema":"{expected_record["schema"]}","status":"{expected_record["status"]}",'
                f'"wgpu_major":30,"stage":"{row["stage"]}","entry":"{row["entry"]}",'
                f'"module_bytes":{row["spirv_bytes"]}}}\n'
            ).encode()
            base.require(row["validation"]["stdout_sha256"] == base.digest_bytes(expected_stdout) and row["validation"]["stdout_bytes"] == len(expected_stdout), "ingestible row stdout bytes changed")
    base.require(base.digest_bytes(base.canonical_json(ids)) == policy["reference_corpus"]["entry_order_sha256"], "wgpu assessment row order changed")
    counts = {name: sum(row["outcome"] == name for row in entries) for name in policy["outcomes"]["order"]}
    base.require(receipt["outcome_counts"] == counts and sum(counts.values()) == len(entries), "wgpu assessment outcome counts changed")
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
    command.add_argument("--port-dir", required=True)
    command.add_argument("--oracle-dir")
    command.add_argument("--dxc-dir", required=True)
    command.add_argument("--dxc-build-dir", required=True)
    command.add_argument("--spirv-val-build-dir", required=True)
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
    sub.add_parser("selftest")
    return result


def selftest() -> None:
    policy = load_policy()
    base.require(base.digest_bytes(KNOWN_STDERR) == policy["outcomes"]["blocked_known"]["stderr_sha256"], "known blocker transcript drift")
    base.require(runtime_readiness([], policy)["runtime_ready"] is False, "M2.5b runtime readiness changed")


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
        else:
            raise base.ArtifactError(f"unknown command {args.command}")
    except (base.ArtifactError, OSError, subprocess.SubprocessError) as error:
        print(f"rt64-wgpu-shader-assessment: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
