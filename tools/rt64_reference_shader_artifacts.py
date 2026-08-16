#!/usr/bin/env python3
"""Build and verify RT64 reference-valid SPIR-V without making a wgpu claim."""

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
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rt64_shader_artifacts as base


ROOT = Path(__file__).resolve().parents[1]
TOOL_PATH = Path(__file__).resolve()
POLICY_PATH = ROOT / "docs/rt64-reference-shader-artifact-schema.json"
RECEIPT_PATH = "reference-receipt.json"
BUILD_RECEIPT_PATH = "spirv-val-build-receipt.json"
BUILD_MANIFEST_PATH = "compiled-source-manifest.json"
GENERATED_AUTHORITY_NAMES = (
    "core_tables_body.inc",
    "core_tables_header.inc",
    "DebugInfo.h",
    "OpenCLDebugInfo100.h",
    "generators.inc",
    "build-version.inc",
)


def require_safe_relative(value: object, label: str) -> PurePosixPath:
    base.require(isinstance(value, str), f"{label} is not text")
    path = PurePosixPath(value)
    base.require(
        path.parts and not path.is_absolute() and ".." not in path.parts,
        f"unsafe {label}",
    )
    base.require(path.as_posix() == value, f"non-canonical {label}")
    return path


def generated_authority_paths(policy: dict) -> tuple[PurePosixPath, ...]:
    expected = tuple(PurePosixPath("build", name) for name in GENERATED_AUTHORITY_NAMES)
    configured = policy["spirv_val"]["generated_authority_files"]
    base.require(isinstance(configured, list), "generated spirv-val authority policy is not a list")
    actual = tuple(
        require_safe_relative(value, f"generated spirv-val authority policy path {index}")
        for index, value in enumerate(configured)
    )
    base.require(actual == expected, "generated spirv-val authority policy denominator changed")
    return expected


def record_generated_authority(output: Path, policy: dict) -> list[dict]:
    records = []
    for relative in generated_authority_paths(policy):
        path = output.joinpath(*relative.parts)
        base.require(path.is_file() and not path.is_symlink(), f"generated spirv-val authority is missing: {relative}")
        records.append({"path": relative.as_posix(), "sha256": base.digest_file(path)})
    return records


def validate_generated_authority(record: object, build_dir: Path, policy: dict) -> None:
    expected = generated_authority_paths(policy)
    base.require(isinstance(record, list) and len(record) == len(expected), "generated spirv-val authority denominator changed")
    for index, (row, relative) in enumerate(zip(record, expected, strict=True)):
        base.require_keys(row, {"path", "sha256"}, f"generated spirv-val authority {index}")
        base.require(row["path"] == relative.as_posix(), "generated spirv-val authority denominator changed")
        path = build_dir.joinpath(*relative.parts)
        base.require(path.is_file() and not path.is_symlink(), f"generated spirv-val authority is missing: {relative}")
        base.require(
            base.digest_file(path) == require_sha(row["sha256"], f"generated spirv-val authority {index}"),
            f"generated spirv-val authority changed: {relative}",
        )


def load_policy() -> dict:
    policy = base.load_json(POLICY_PATH)
    base.require_keys(
        policy,
        {
            "schema",
            "direct_consumers",
            "receipt_schema",
            "spirv_val_build_receipt_schema",
            "spirv_val_smoke_receipt_schema",
            "spirv_val_smoke_claim_boundary",
            "artifact_producer",
            "artifact_policy",
            "dxc",
            "spirv_val",
            "required_validation",
            "maximum_receipt_bytes",
            "claim_boundary",
        },
        "reference shader policy",
    )
    base.require(
        policy["schema"] == "fn64.rt64-reference-shader-policy.v1",
        "unsupported reference shader policy",
    )
    base.require(
        policy["direct_consumers"]
        == [
            "tools/rt64_reference_shader_artifacts.py",
            "tools/test_rt64_reference_shader_artifacts.py",
        ],
        "reference policy consumer denominator changed",
    )
    base.require_keys(
        policy["dxc"],
        {
            "commit",
            "spirv_tools_path",
            "spirv_tools_commit",
            "spirv_tools_license",
            "spirv_tools_license_path",
            "spirv_tools_license_sha256",
            "spirv_headers_path",
            "spirv_headers_commit",
            "inventory_grammar_path",
            "inventory_grammar_sha256",
            "grammar_files",
            "registry_file",
        },
        "reference DXC policy",
    )
    base.require_keys(
        policy["spirv_val"],
        {
            "source_subdirectory",
            "header_subdirectory",
            "generator",
            "flags",
            "dynamic_tool_bindings",
            "target",
            "parallel",
            "forced_build_version_description",
            "source_date_epoch",
            "version_prefix",
            "controlled_environment_names",
            "generated_authority_files",
            "candidate_paths",
            "validation_arguments",
            "maximum_binary_bytes",
            "maximum_build_manifest_bytes",
            "maximum_smoke_inventory_rows",
            "darwin_runtime_closure",
        },
        "spirv-val policy",
    )
    base.require_keys(
        policy["spirv_val"]["darwin_runtime_closure"],
        {"inspector", "format", "system_load_names"},
        "spirv-val Darwin runtime policy",
    )
    base.require(
        policy["spirv_val"]["controlled_environment_names"]
        == [
            "PATH",
            "LC_ALL",
            "LANG",
            "CC",
            "CXX",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_NOSYSTEM",
            "GIT_NO_REPLACE_OBJECTS",
            "GIT_OPTIONAL_LOCKS",
            "GIT_TERMINAL_PROMPT",
            "FORCED_BUILD_VERSION_DESCRIPTION",
            "SOURCE_DATE_EPOCH",
        ],
        "spirv-val controlled environment denominator changed",
    )
    generated_authority_paths(policy)
    base.require(
        policy["spirv_val"]["maximum_smoke_inventory_rows"] == 65536,
        "SPIR-V smoke inventory row denominator changed",
    )
    for record, expected_path, label in (
        (policy["artifact_producer"], base.TOOL_PATH, "artifact producer"),
        (policy["artifact_policy"], base.POLICY_PATH, "artifact policy"),
    ):
        base.require_keys(record, {"path", "sha256"}, label)
        relative = require_safe_relative(record["path"], f"{label} path")
        base.require(ROOT.joinpath(*relative.parts) == expected_path, f"{label} path changed")
        base.require(
            base.SHA256_RE.fullmatch(record.get("sha256", "")) is not None
            and base.digest_file(expected_path) == record["sha256"],
            f"{label} identity changed",
        )
    base_policy = base.load_policy()
    dxc = policy["dxc"]
    base.require(dxc["commit"] == base_policy["dxc"]["commit"], "reference DXC pin drift")
    dependencies = {row["path"]: row for row in base_policy["dxc"]["source_dependencies"]}
    base.require(
        dxc["spirv_tools_commit"] == dependencies[dxc["spirv_tools_path"]]["commit"],
        "SPIRV-Tools pin drift",
    )
    base.require(
        dxc["spirv_headers_commit"] == dependencies[dxc["spirv_headers_path"]]["commit"],
        "SPIRV-Headers pin drift",
    )
    base.require(
        dxc["spirv_tools_license_sha256"]
        == dependencies[dxc["spirv_tools_path"]]["license_files"][0]["sha256"],
        "SPIRV-Tools license pin drift",
    )
    base.require(policy["claim_boundary"] == "reference-valid-only-not-wgpu-runtime-or-parity", "reference claim boundary drift")
    base.require(
        policy["spirv_val_smoke_receipt_schema"] == "fn64.spirv-val-single-artifact-smoke.v1",
        "unsupported SPIR-V smoke receipt schema",
    )
    base.require(
        policy["spirv_val_smoke_claim_boundary"]
        == "qualified-validator-single-artifact-reference-valid-and-inventoried-not-artifact-provenance-corpus-wgpu-runtime-or-parity",
        "SPIR-V smoke claim boundary drift",
    )
    base.require(
        policy["required_validation"]
        == [
            "dxc-built-in-spirv-validation",
            "spirv-tools-spirv-val-vulkan1.0",
            "grammar-bound-capability-extension-nonuniform-inventory",
        ],
        "reference validation denominator changed",
    )
    return policy


def source_records(source: Path, policy: dict) -> tuple[dict, dict]:
    audit = base.validate_dxc_source(source, require_complete=True)
    by_path = {row["path"]: row for row in audit["source_dependencies"]}
    dxc = policy["dxc"]
    tools = copy.deepcopy(by_path[dxc["spirv_tools_path"]])
    headers = copy.deepcopy(by_path[dxc["spirv_headers_path"]])
    base.require(tools["commit"] == dxc["spirv_tools_commit"], "SPIRV-Tools source identity changed")
    base.require(headers["commit"] == dxc["spirv_headers_commit"], "SPIRV-Headers source identity changed")
    grammar_rows = dxc["grammar_files"]
    base.require(isinstance(grammar_rows, list) and len(grammar_rows) == 16, "SPIR-V grammar denominator changed")
    seen_grammars = set()
    for index, row in enumerate(grammar_rows):
        base.require_keys(row, {"path", "sha256"}, f"SPIR-V grammar row {index}")
        grammar_relative = require_safe_relative(row["path"], f"SPIR-V grammar row {index} path")
        base.require(grammar_relative.as_posix() not in seen_grammars, "SPIR-V grammar path repeated")
        seen_grammars.add(grammar_relative.as_posix())
        base.require(base.digest_file(source.joinpath(*grammar_relative.parts)) == row["sha256"], f"SPIR-V grammar bytes changed: {grammar_relative.name}")
    inventory_grammar = require_safe_relative(dxc["inventory_grammar_path"], "inventory grammar path")
    base.require(
        dxc["inventory_grammar_sha256"]
        == next(row["sha256"] for row in grammar_rows if row["path"] == inventory_grammar.as_posix()),
        "inventory grammar is outside the build grammar denominator",
    )
    registry = dxc["registry_file"]
    base.require_keys(registry, {"path", "sha256"}, "SPIR-V registry row")
    registry_relative = require_safe_relative(registry["path"], "SPIR-V registry path")
    base.require(base.digest_file(source.joinpath(*registry_relative.parts)) == registry["sha256"], "SPIR-V registry bytes changed")
    return audit, {
        "spirv_tools": tools,
        "spirv_headers": headers,
        "grammar_files": copy.deepcopy(grammar_rows),
        "registry_file": copy.deepcopy(registry),
    }


@dataclass(frozen=True)
class SpirvValClosure:
    root: Path
    binary: base.ContainedExecutable
    inspector: Path
    receipt_record: dict
    grammar: Path
    grammar_sha256: str


def select_spirv_val(output: Path, policy: dict) -> base.ContainedExecutable:
    candidates = [output.joinpath(*PurePosixPath(row).parts) for row in policy["spirv_val"]["candidate_paths"]]
    present = [path for path in candidates if os.path.lexists(path)]
    base.require(len(present) == 1, "SPIRV-Tools build did not emit exactly one reviewed spirv-val path")
    binary = base.qualify_contained_executable(
        output,
        present[0],
        policy["spirv_val"]["maximum_binary_bytes"],
        "spirv-val",
    )
    base.require(binary.receipt_record["kind"] == "regular", "spirv-val must be one regular retained file")
    return binary


def qualify_spirv_val_closure(output: Path, source: Path, policy: dict) -> SpirvValClosure:
    base.require(sys.platform == "darwin", "spirv-val qualification is currently implemented only for macOS")
    runtime = policy["spirv_val"]["darwin_runtime_closure"]
    base.require(runtime["format"] == "otool-L-v1", "spirv-val loader format changed")
    inspector = base.executable(runtime["inspector"])
    binary = select_spirv_val(output, policy)
    rows, transcript = base.inspect_otool_loads(inspector, binary.target_path, "spirv-val")
    loads = base.classify_macho_loads(rows, {}, set(runtime["system_load_names"]), "spirv-val")
    record = {
        "platform": "darwin",
        "format": runtime["format"],
        "binary_artifact": binary.receipt_record,
        "loads": loads,
        "inspection": transcript,
    }
    record["closure_sha256"] = base.digest_bytes(base.canonical_json(record))
    grammar_relative = require_safe_relative(policy["dxc"]["inventory_grammar_path"], "SPIR-V grammar path")
    grammar = source.joinpath(*grammar_relative.parts)
    return SpirvValClosure(
        output,
        binary,
        inspector,
        record,
        grammar,
        policy["dxc"]["inventory_grammar_sha256"],
    )


@contextmanager
def staged_spirv_val(closure: SpirvValClosure, parent: Path):
    policy = load_policy()
    with tempfile.TemporaryDirectory(prefix=".fn64-spirv-val-runtime-", dir=parent) as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        binary = root / "bin/spirv-val"
        grammar = root / "share/spirv.core.grammar.json"
        base.stage_qualified_file(
            closure.binary.target_path,
            closure.binary.receipt_record["target_sha256"],
            binary,
            policy["spirv_val"]["maximum_binary_bytes"],
            0o500,
            "spirv-val",
        )
        base.stage_qualified_file(
            closure.grammar,
            closure.grammar_sha256,
            grammar,
            base.load_policy()["git_checkout_maximum_file_bytes"],
            0o400,
            "SPIRV-Headers grammar",
        )
        yield binary, grammar


def spirv_val_build_environment(
    cmake: Path,
    ninja: Path,
    python: Path,
    git_tool: Path,
    cc: Path,
    cxx: Path,
    policy: dict,
) -> dict[str, str]:
    directories = []
    for tool in (cmake, ninja, python, git_tool, cc, cxx):
        directory = str(tool.parent)
        if directory not in directories:
            directories.append(directory)
    for directory in ("/usr/bin", "/bin"):
        if directory not in directories:
            directories.append(directory)
    environment = {
        "PATH": os.pathsep.join(directories),
        "LC_ALL": "C",
        "LANG": "C",
        "CC": str(cc),
        "CXX": str(cxx),
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_NO_REPLACE_OBJECTS": "1",
        "GIT_OPTIONAL_LOCKS": "0",
        "GIT_TERMINAL_PROMPT": "0",
        "FORCED_BUILD_VERSION_DESCRIPTION": policy["spirv_val"]["forced_build_version_description"],
        "SOURCE_DATE_EPOCH": policy["spirv_val"]["source_date_epoch"],
    }
    base.require(
        list(environment) == policy["spirv_val"]["controlled_environment_names"],
        "spirv-val controlled environment order changed",
    )
    return environment


def environment_record(environment: dict[str, str], policy: dict) -> list[dict]:
    base.require(
        list(environment) == policy["spirv_val"]["controlled_environment_names"],
        "spirv-val controlled environment names changed",
    )
    return [
        {"name": name, "value_sha256": base.digest_bytes(value.encode())}
        for name, value in environment.items()
    ]


def validate_environment_record(record: object, environment: dict[str, str], policy: dict) -> None:
    base.require(isinstance(record, list), "spirv-val controlled environment record is not a list")
    for index, row in enumerate(record):
        base.require_keys(row, {"name", "value_sha256"}, f"spirv-val controlled environment row {index}")
        base.require(isinstance(row["name"], str), f"spirv-val environment name is not text at row {index}")
        require_sha(row["value_sha256"], f"spirv-val environment {row['name']}")
    base.require(
        record == environment_record(environment, policy),
        "spirv-val controlled environment record changed",
    )


def build_spirv_val(args: argparse.Namespace) -> dict:
    source = Path(args.dxc_dir).resolve()
    output = Path(args.output_dir).resolve()
    base.require(not output.exists(), "spirv-val output directory must not already exist")
    base.require(ROOT not in output.parents and output != ROOT, "spirv-val build must stay outside fn64")
    policy = load_policy()
    source_audit, authorities = source_records(source, policy)
    cmake = base.executable(args.cmake)
    ninja = base.executable(args.ninja)
    python = base.executable(args.python)
    git_tool = base.executable(args.git)
    cc = base.executable(args.cc)
    cxx = base.executable(args.cxx)
    output.mkdir(parents=True)
    build = output / "build"
    source_subdir = source.joinpath(*PurePosixPath(policy["spirv_val"]["source_subdirectory"]).parts)
    header_subdir = source.joinpath(*PurePosixPath(policy["spirv_val"]["header_subdirectory"]).parts)
    env = spirv_val_build_environment(cmake, ninja, python, git_tool, cc, cxx, policy)
    configure = [
        str(cmake),
        "-S",
        str(source_subdir),
        "-B",
        str(build),
        "-G",
        policy["spirv_val"]["generator"],
        *policy["spirv_val"]["flags"],
        f"-DPython3_EXECUTABLE={python}",
        f"-DSPIRV-Headers_SOURCE_DIR={header_subdir}",
    ]
    configure_log = base.run_logged(configure, output, env)
    build_command = [
        str(cmake),
        "--build",
        str(build),
        "--target",
        policy["spirv_val"]["target"],
        "--parallel",
        str(policy["spirv_val"]["parallel"]),
    ]
    build_log = base.run_logged(build_command, output, env)
    command_graph = base.run_logged(
        [str(ninja), "-C", str(build), "-t", "commands", policy["spirv_val"]["target"]],
        output,
        env,
    )
    post_audit, post_authorities = source_records(source, policy)
    base.require(post_audit == source_audit and post_authorities == authorities, "SPIR-V authority changed during build")
    closure = qualify_spirv_val_closure(output, source, policy)
    compile_commands = build / "compile_commands.json"
    cache = build / "CMakeCache.txt"
    ninja_file = build / "build.ninja"
    ninja_log = build / ".ninja_log"
    for path in (compile_commands, cache, ninja_file, ninja_log):
        base.require(path.is_file(), f"spirv-val build evidence is missing: {path.name}")
    manifest = base.compiled_source_manifest(source, build, compile_commands, ninja_log)
    components = {row["component"] for row in manifest["translation_units"]}
    base.require("spirv-tools-apache-2.0" in components, "spirv-val target compiled no SPIRV-Tools source")
    base.require(
        components <= {"spirv-tools-apache-2.0", "official-cmake-generated-source"},
        f"unreviewed source component entered spirv-val: {sorted(components)}",
    )
    manifest_path = output / BUILD_MANIFEST_PATH
    manifest_path.write_bytes(base.pretty_json(manifest))
    generated_authority = record_generated_authority(output, policy)
    with staged_spirv_val(closure, output) as (validator, _):
        validator_record = base.tool_record(
            validator,
            ["--version"],
            {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"},
        )
    base.require(
        validator_record["version_stdout"].startswith(policy["spirv_val"]["version_prefix"]),
        "spirv-val source build has an unexpected version identity",
    )
    tools = {
        "cmake": base.tool_record(cmake, ["--version"]),
        "ninja": base.tool_record(ninja, ["--version"]),
        "python": base.tool_record(python, ["--version"]),
        "git": base.tool_record(git_tool, ["--version"]),
        "cc": base.tool_record(cc, ["--version"]),
        "cxx": base.tool_record(cxx, ["--version"]),
        "runtime_dependency_inspector": base.tool_record(
            closure.inspector,
            ["--version"],
            {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"},
        ),
        "spirv_val": validator_record,
    }
    receipt = base.add_receipt_hash(
        {
            "schema": policy["spirv_val_build_receipt_schema"],
            "status": "complete",
            "orchestration_producer_sha256": base.digest_file(TOOL_PATH),
            "artifact_producer_sha256": base.digest_file(base.TOOL_PATH),
            "reference_policy_sha256": base.digest_file(POLICY_PATH),
            "artifact_policy_sha256": base.digest_file(base.POLICY_PATH),
            "source": source_audit,
            "authorities": authorities,
            "configuration": {
                "generator": policy["spirv_val"]["generator"],
                "flags": policy["spirv_val"]["flags"],
                "target": policy["spirv_val"]["target"],
                "parallel": policy["spirv_val"]["parallel"],
                "dynamic_tool_bindings": policy["spirv_val"]["dynamic_tool_bindings"],
                "environment": environment_record(env, policy),
            },
            "tools": tools,
            "configure": configure_log,
            "build": build_log,
            "command_graph": command_graph,
            "cmake_cache_sha256": base.digest_file(cache),
            "build_ninja_sha256": base.digest_file(ninja_file),
            "ninja_log_sha256": base.digest_file(ninja_log),
            "compile_commands_sha256": base.digest_file(compile_commands),
            "compiled_source_manifest": {
                "path": BUILD_MANIFEST_PATH,
                "sha256": base.digest_file(manifest_path),
                "source_set_sha256": manifest["source_set_sha256"],
                "translation_units": len(manifest["translation_units"]),
                "counts_by_component": manifest["counts_by_component"],
            },
            "generated_authority": generated_authority,
            "validator_closure": closure.receipt_record,
            "validator_sha256": closure.binary.receipt_record["target_sha256"],
            "claim_boundary": "local-source-build-integrity-not-transferable-process-attestation",
        }
    )
    base.require(not base.LOCAL_PATH_RE.search(json.dumps(receipt)), "spirv-val receipt leaked a machine-local path")
    (output / BUILD_RECEIPT_PATH).write_bytes(base.pretty_json(receipt))
    return receipt


def validate_spirv_val_build(build_dir: Path, source: Path) -> tuple[dict, SpirvValClosure]:
    policy = load_policy()
    receipt = base.load_canonical_json(
        build_dir / BUILD_RECEIPT_PATH,
        policy["maximum_receipt_bytes"],
        "spirv-val build receipt",
    )
    base.require_keys(
        receipt,
        {
            "schema",
            "status",
            "orchestration_producer_sha256",
            "artifact_producer_sha256",
            "reference_policy_sha256",
            "artifact_policy_sha256",
            "source",
            "authorities",
            "configuration",
            "tools",
            "configure",
            "build",
            "command_graph",
            "cmake_cache_sha256",
            "build_ninja_sha256",
            "ninja_log_sha256",
            "compile_commands_sha256",
            "compiled_source_manifest",
            "generated_authority",
            "validator_closure",
            "validator_sha256",
            "claim_boundary",
            "receipt_sha256",
        },
        "spirv-val build receipt",
    )
    base.require(receipt["schema"] == policy["spirv_val_build_receipt_schema"] and receipt["status"] == "complete", "spirv-val build receipt incomplete")
    base.validate_receipt_hash(receipt)
    base.require(receipt["orchestration_producer_sha256"] == base.digest_file(TOOL_PATH), "spirv-val build used another orchestration producer")
    base.require(receipt["artifact_producer_sha256"] == base.digest_file(base.TOOL_PATH), "spirv-val build bound another artifact producer")
    base.require(receipt["reference_policy_sha256"] == base.digest_file(POLICY_PATH), "reference policy identity mismatch")
    base.require(receipt["artifact_policy_sha256"] == base.digest_file(base.POLICY_PATH), "artifact policy identity mismatch")
    audit, authorities = source_records(source, policy)
    base.require(receipt["source"] == audit and receipt["authorities"] == authorities, "spirv-val source authority changed")
    expected_configuration = {
        "generator": policy["spirv_val"]["generator"],
        "flags": policy["spirv_val"]["flags"],
        "target": policy["spirv_val"]["target"],
        "parallel": policy["spirv_val"]["parallel"],
        "dynamic_tool_bindings": policy["spirv_val"]["dynamic_tool_bindings"],
    }
    base.require_keys(
        receipt["configuration"],
        {"generator", "flags", "target", "parallel", "dynamic_tool_bindings", "environment"},
        "spirv-val configuration",
    )
    for key, value in expected_configuration.items():
        base.require(receipt.get("configuration", {}).get(key) == value, f"spirv-val configuration changed: {key}")
    base.require_keys(receipt["tools"], {"cmake", "ninja", "python", "git", "cc", "cxx", "runtime_dependency_inspector", "spirv_val"}, "spirv-val tool closure")
    for name, record in receipt["tools"].items():
        base.validate_tool_record(record, f"spirv-val {name} record")
    qualified_build_tools = {}
    for name in ("cmake", "ninja", "python", "git", "cc", "cxx"):
        record = receipt["tools"][name]
        path = base.executable(record["name"])
        base.require(
            record == base.tool_record(path, record["version_arguments"]),
            f"spirv-val {name} tool identity changed",
        )
        qualified_build_tools[name] = path
    expected_environment = spirv_val_build_environment(
        qualified_build_tools["cmake"],
        qualified_build_tools["ninja"],
        qualified_build_tools["python"],
        qualified_build_tools["git"],
        qualified_build_tools["cc"],
        qualified_build_tools["cxx"],
        policy,
    )
    validate_environment_record(receipt["configuration"]["environment"], expected_environment, policy)
    for name in ("configure", "build", "command_graph"):
        base.validate_log_record(receipt[name], f"spirv-val {name} transcript")
    for path, key in (
        (build_dir / "build/CMakeCache.txt", "cmake_cache_sha256"),
        (build_dir / "build/build.ninja", "build_ninja_sha256"),
        (build_dir / "build/.ninja_log", "ninja_log_sha256"),
        (build_dir / "build/compile_commands.json", "compile_commands_sha256"),
    ):
        base.require(base.digest_file(path) == receipt[key], f"spirv-val build graph changed: {path.name}")
    manifest_record = receipt["compiled_source_manifest"]
    base.require_keys(manifest_record, {"path", "sha256", "source_set_sha256", "translation_units", "counts_by_component"}, "spirv-val source manifest record")
    base.require(manifest_record["path"] == BUILD_MANIFEST_PATH, "spirv-val source manifest path changed")
    manifest_path = build_dir / BUILD_MANIFEST_PATH
    base.require(base.digest_file(manifest_path) == manifest_record["sha256"], "spirv-val source manifest changed")
    manifest = base.load_canonical_json(manifest_path, policy["spirv_val"]["maximum_build_manifest_bytes"], "spirv-val source manifest")
    reconstructed = base.compiled_source_manifest(
        source,
        build_dir / "build",
        build_dir / "build/compile_commands.json",
        build_dir / "build/.ninja_log",
    )
    base.require(manifest == reconstructed, "spirv-val source manifest does not match executed target")
    base.validate_compiled_source_files(manifest, source, build_dir / "build")
    base.require(manifest_record["source_set_sha256"] == manifest["source_set_sha256"], "spirv-val source-set mismatch")
    base.require(manifest_record["translation_units"] == len(manifest["translation_units"]), "spirv-val translation-unit count mismatch")
    base.require(manifest_record["counts_by_component"] == manifest["counts_by_component"], "spirv-val component counts mismatch")
    validate_generated_authority(receipt["generated_authority"], build_dir, policy)
    closure = qualify_spirv_val_closure(build_dir, source, policy)
    base.require(receipt["validator_closure"] == closure.receipt_record, "spirv-val runtime closure changed")
    base.require(receipt["validator_sha256"] == closure.binary.receipt_record["target_sha256"], "spirv-val binary identity changed")
    with staged_spirv_val(closure, build_dir) as (validator, _):
        validator_record = base.tool_record(validator, ["--version"], {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"})
    base.require(
        validator_record["version_stdout"].startswith(policy["spirv_val"]["version_prefix"]),
        "spirv-val version identity changed",
    )
    base.require(receipt["tools"]["spirv_val"] == validator_record, "spirv-val protocol identity changed")
    base.require(
        receipt["tools"]["runtime_dependency_inspector"]
        == base.tool_record(closure.inspector, ["--version"], {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"}),
        "spirv-val loader inspector changed",
    )
    base.require(receipt["claim_boundary"] == "local-source-build-integrity-not-transferable-process-attestation", "spirv-val build claim changed")
    return receipt, closure


def decode_literal_string(words: list[int], label: str) -> str:
    data = bytearray()
    for word_index, word in enumerate(words):
        chunk = word.to_bytes(4, "little")
        if b"\0" in chunk:
            index = chunk.index(0)
            base.require(word_index == len(words) - 1, f"{label} has trailing words after its terminator")
            base.require(not any(chunk[index + 1 :]), f"{label} has nonzero string padding")
            data.extend(chunk[:index])
            break
        data.extend(chunk)
    else:
        raise base.ArtifactError(f"{label} is not NUL terminated")
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError as error:
        raise base.ArtifactError(f"{label} is not UTF-8") from error


def grammar_tables(grammar_bytes: bytes) -> dict:
    try:
        grammar = json.loads(grammar_bytes)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise base.ArtifactError("SPIR-V grammar is not valid UTF-8 JSON") from error
    base.require(isinstance(grammar, dict), "SPIR-V grammar root is not an object")
    instructions = grammar.get("instructions")
    operand_kinds = grammar.get("operand_kinds")
    base.require(isinstance(instructions, list) and isinstance(operand_kinds, list), "SPIR-V grammar tables are absent")
    opcodes: dict[str, int] = {}
    for name in ("OpCapability", "OpExtension", "OpDecorate", "OpMemberDecorate", "OpDecorationGroup", "OpGroupDecorate", "OpGroupMemberDecorate"):
        rows = [row for row in instructions if isinstance(row, dict) and row.get("opname") == name]
        base.require(len(rows) == 1 and isinstance(rows[0].get("opcode"), int), f"SPIR-V grammar lacks exact {name}")
        opcodes[name] = rows[0]["opcode"]
    base.require(len(set(opcodes.values())) == len(opcodes), "SPIR-V inventory opcodes overlap")
    enums = {}
    for kind in ("Capability", "Decoration"):
        rows = [row for row in operand_kinds if isinstance(row, dict) and row.get("kind") == kind]
        base.require(len(rows) == 1 and rows[0].get("category") == "ValueEnum", f"SPIR-V grammar lacks {kind} enum")
        values: dict[int, str] = {}
        for row in rows[0].get("enumerants", []):
            value = row.get("value")
            name = row.get("enumerant")
            base.require(isinstance(value, int) and isinstance(name, str), f"malformed SPIR-V {kind} enum")
            base.require(value not in values, f"duplicate SPIR-V {kind} value")
            values[value] = name
        enums[kind] = values
    base.require(enums["Decoration"].get(5300) == "NonUniform", "SPIR-V NonUniform grammar value changed")
    base.require(enums["Capability"].get(5301) == "ShaderNonUniform", "SPIR-V ShaderNonUniform grammar value changed")
    return {"opcodes": opcodes, "enums": enums}


def inventory_spirv(artifact_bytes: bytes, grammar_bytes: bytes, maximum_rows: int | None = None) -> dict:
    base.require(
        maximum_rows is None or isinstance(maximum_rows, int) and not isinstance(maximum_rows, bool) and maximum_rows > 0,
        "SPIR-V inventory row limit is invalid",
    )
    base.require(len(artifact_bytes) >= 20 and len(artifact_bytes) % 4 == 0, "SPIR-V module has an invalid byte extent")
    words = list(struct.unpack(f"<{len(artifact_bytes) // 4}I", artifact_bytes))
    base.require(words[0] == 0x07230203, "SPIR-V magic mismatch")
    bound = words[3]
    base.require(bound >= 1, "SPIR-V id bound must be at least one")
    base.require(words[4] == 0, "SPIR-V schema word must be zero")
    tables = grammar_tables(grammar_bytes)
    opcodes = tables["opcodes"]
    capabilities = []
    extensions = []
    non_uniform = []

    def enforce_row_budget() -> None:
        if maximum_rows is not None:
            base.require(
                len(capabilities) + len(extensions) + len(non_uniform) <= maximum_rows,
                "SPIR-V semantic inventory exceeds the smoke row budget",
            )

    offset = 5
    while offset < len(words):
        first = words[offset]
        word_count = first >> 16
        opcode = first & 0xFFFF
        base.require(word_count > 0 and offset + word_count <= len(words), f"malformed SPIR-V instruction at word {offset}")
        operands = words[offset + 1 : offset + word_count]
        if opcode in {opcodes["OpDecorationGroup"], opcodes["OpGroupDecorate"], opcodes["OpGroupMemberDecorate"]}:
            raise base.ArtifactError(f"SPIR-V group decoration is not implemented at word {offset}")
        if opcode == opcodes["OpCapability"]:
            base.require(word_count == 2, f"malformed OpCapability at word {offset}")
            value = operands[0]
            name = tables["enums"]["Capability"].get(value)
            base.require(name is not None, f"unknown SPIR-V capability {value} at word {offset}")
            capabilities.append({"name": name, "value": value, "word_offset": offset})
            enforce_row_budget()
        elif opcode == opcodes["OpExtension"]:
            base.require(word_count >= 2, f"malformed OpExtension at word {offset}")
            extensions.append({"name": decode_literal_string(operands, f"OpExtension at word {offset}"), "word_offset": offset})
            enforce_row_budget()
        elif opcode == opcodes["OpDecorate"]:
            base.require(word_count >= 3, f"malformed OpDecorate at word {offset}")
            base.require(0 < operands[0] < bound, f"OpDecorate target id is outside the module bound at word {offset}")
            decoration = tables["enums"]["Decoration"].get(operands[1])
            base.require(decoration is not None, f"unknown SPIR-V decoration {operands[1]} at word {offset}")
            if decoration == "NonUniform":
                base.require(word_count == 3, f"NonUniform decoration has operands at word {offset}")
                non_uniform.append({"target_id": operands[0], "word_offset": offset})
                enforce_row_budget()
        elif opcode == opcodes["OpMemberDecorate"]:
            base.require(word_count >= 4, f"malformed OpMemberDecorate at word {offset}")
            base.require(0 < operands[0] < bound, f"OpMemberDecorate target id is outside the module bound at word {offset}")
            decoration = tables["enums"]["Decoration"].get(operands[2])
            base.require(decoration is not None, f"unknown SPIR-V member decoration {operands[2]} at word {offset}")
            base.require(decoration != "NonUniform", f"NonUniform member decoration is not implemented at word {offset}")
        offset += word_count
    base.require(offset == len(words), "SPIR-V instruction stream did not terminate exactly")
    result = {
        "schema": "fn64.spirv-semantic-inventory.v1",
        "word_count": len(words),
        "id_bound": bound,
        "capabilities": capabilities,
        "extensions": extensions,
        "non_uniform_decorations": non_uniform,
    }
    result["inventory_sha256"] = base.digest_bytes(base.canonical_json(result))
    return result


def run_spirv_val(validator: Path, artifact: Path, expected: dict, output: Path, policy: dict) -> dict:
    base.require(
        policy["spirv_val"]["validation_arguments"] == ["--target-env", "vulkan1.0"],
        "spirv-val invocation admits noncanonical or relaxed arguments",
    )
    before_outputs = base.output_entry_set(output)
    before = base.stable_file_bytes(artifact, base.load_policy()["spirv"]["maximum_artifact_bytes"], f"SPIR-V before spirv-val {expected['id']}")
    result = subprocess.run(
        [str(validator), *policy["spirv_val"]["validation_arguments"], "-"],
        input=before,
        capture_output=True,
        env={"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"},
    )
    base.require(result.returncode == 0, f"spirv-val failed for {expected['id']}: {result.stderr.decode(errors='replace')[-2000:]}")
    base.require(not result.stdout and not result.stderr, f"spirv-val emitted unexpected output for {expected['id']}")
    base.require(base.output_entry_set(output) == before_outputs, f"spirv-val changed the artifact output set: {expected['id']}")
    after = base.stable_file_bytes(artifact, base.load_policy()["spirv"]["maximum_artifact_bytes"], f"SPIR-V after spirv-val {expected['id']}")
    base.require(after == before, f"spirv-val changed SPIR-V bytes: {expected['id']}")
    return {
        "status": "passed",
        "arguments": [*policy["spirv_val"]["validation_arguments"], "-"],
        "input_sha256": base.digest_bytes(before),
        "input_bytes": len(before),
        "stdout_sha256": base.digest_bytes(result.stdout),
        "stderr_sha256": base.digest_bytes(result.stderr),
    }


@dataclass(frozen=True)
class QualifiedExternalSpirv:
    artifact_bytes: bytes
    directory: Path
    directory_identity: tuple[int, int, int, int, int, int, int]
    parent_identities: tuple[tuple[Path, tuple[int, int, int, int, int, int, int]], ...]


def read_external_spirv(path_text: object) -> QualifiedExternalSpirv:
    base.require(isinstance(path_text, str) and path_text, "SPIR-V smoke artifact path is empty")
    path = Path(path_text)
    base.require(path.is_absolute() and ".." not in path.parts, "SPIR-V smoke artifact path must be absolute without traversal")
    resolved = path.resolve(strict=True)
    base.require(ROOT != resolved and ROOT not in resolved.parents, "SPIR-V smoke artifact must stay outside fn64")
    parents = base.contained_parent_identities(Path(path.anchor), path, "SPIR-V smoke artifact")
    artifact_bytes, artifact_info = base.stable_regular_bytes(
        path,
        base.load_policy()["spirv"]["maximum_artifact_bytes"],
        "SPIR-V smoke artifact",
    )
    base.require(artifact_info.st_nlink == 1, "SPIR-V smoke artifact has another hardlink")
    base.verify_parent_identities(parents, "SPIR-V smoke artifact")
    base.require(path.resolve(strict=True) == resolved, "SPIR-V smoke artifact target changed while it was qualified")
    base.require(parents[-1][0] == path.parent, "SPIR-V smoke artifact directory was not qualified")
    return QualifiedExternalSpirv(
        artifact_bytes,
        resolved.parent,
        parents[-1][1],
        tuple(parents),
    )


def verify_identity_rows(
    rows: tuple[tuple[Path, tuple[int, int, int, int, int, int, int]], ...],
    label: str,
) -> None:
    try:
        for path, expected in rows:
            info = path.lstat()
            base.require(
                stat.S_ISDIR(info.st_mode)
                and not path.is_symlink()
                and (info.st_dev, info.st_ino) == expected[:2],
                f"{label} parent directory identity changed while it was qualified",
            )
    except OSError as error:
        raise base.ArtifactError(f"{label} parent path disappeared while it was qualified: {error}") from error


def qualify_smoke_staging_root(private_root: Path, artifact: QualifiedExternalSpirv) -> tuple:
    staging_rows = tuple(
        base.contained_parent_identities(
            Path(private_root.anchor),
            private_root / ".fn64-stage-boundary",
            "SPIR-V smoke staging",
        )
    )
    artifact_object = artifact.directory_identity[:2]
    base.require(
        all(identity[:2] != artifact_object for _, identity in staging_rows),
        "SPIR-V smoke staging contains the qualified artifact directory identity",
    )
    verify_identity_rows(artifact.parent_identities, "SPIR-V smoke artifact")
    verify_identity_rows(staging_rows, "SPIR-V smoke staging")
    return staging_rows


def smoke_spirv_val(args: argparse.Namespace) -> dict:
    policy = load_policy()
    source = Path(args.dxc_dir).resolve()
    build_dir = Path(args.build_dir).resolve()
    build_receipt, closure = validate_spirv_val_build(build_dir, source)
    artifact = read_external_spirv(args.artifact)
    with tempfile.TemporaryDirectory(prefix="fn64-spirv-val-smoke-") as temporary:
        private_root = Path(temporary).resolve(strict=True)
        base.require(
            private_root != ROOT and ROOT not in private_root.parents,
            "SPIR-V smoke staging overlaps fn64",
        )
        base.require(
            private_root != artifact.directory
            and artifact.directory not in private_root.parents
            and private_root not in artifact.directory.parents,
            "SPIR-V smoke staging overlaps the artifact directory tree",
        )
        qualify_smoke_staging_root(private_root, artifact)
        private_root.chmod(0o700)
        staging_rows = qualify_smoke_staging_root(private_root, artifact)
        base.require(stat.S_IMODE(private_root.stat().st_mode) == 0o700, "SPIR-V smoke staging root is not private")
        verify_identity_rows(artifact.parent_identities, "SPIR-V smoke artifact")
        verify_identity_rows(staging_rows, "SPIR-V smoke staging")
        input_snapshot = private_root / "input.spv"
        base.write_new_private_file(input_snapshot, artifact.artifact_bytes)
        with staged_spirv_val(closure, private_root) as (validator, grammar_path):
            grammar_bytes = base.stable_file_bytes(
                grammar_path,
                base.load_policy()["git_checkout_maximum_file_bytes"],
                "staged SPIR-V smoke grammar",
            )
            base.require(base.digest_bytes(grammar_bytes) == closure.grammar_sha256, "staged SPIR-V smoke grammar changed")
            validation = run_spirv_val(validator, input_snapshot, {"id": "explicit-witness"}, private_root, policy)
            base.require(
                validation["input_sha256"] == base.digest_bytes(artifact.artifact_bytes)
                and validation["input_bytes"] == len(artifact.artifact_bytes),
                "SPIR-V smoke validation input changed",
            )
            inventory = inventory_spirv(
                artifact.artifact_bytes,
                grammar_bytes,
                maximum_rows=policy["spirv_val"]["maximum_smoke_inventory_rows"],
            )
    shader_non_uniform = sum(row["name"] == "ShaderNonUniform" for row in inventory["capabilities"])
    non_uniform = len(inventory["non_uniform_decorations"])
    required = bool(args.require_shader_nonuniform)
    satisfied = shader_non_uniform > 0 and non_uniform > 0
    if required:
        base.require(shader_non_uniform > 0, "SPIR-V smoke artifact lacks ShaderNonUniform capability")
        base.require(non_uniform > 0, "SPIR-V smoke artifact lacks a NonUniform decoration")
    result = base.add_receipt_hash(
        {
            "schema": policy["spirv_val_smoke_receipt_schema"],
            "status": "passed",
            "orchestration_producer_sha256": base.digest_file(TOOL_PATH),
            "reference_policy_sha256": base.digest_file(POLICY_PATH),
            "spirv_val_build_receipt_sha256": build_receipt["receipt_sha256"],
            "validator_sha256": build_receipt["validator_sha256"],
            "grammar_sha256": closure.grammar_sha256,
            "artifact_sha256": base.digest_bytes(artifact.artifact_bytes),
            "artifact_bytes": len(artifact.artifact_bytes),
            "validation": validation,
            "semantic_inventory": inventory,
            "shader_nonuniform_witness": {
                "required": required,
                "capability_count": shader_non_uniform,
                "decoration_count": non_uniform,
                "satisfied": satisfied,
            },
            "claim_boundary": policy["spirv_val_smoke_claim_boundary"],
        }
    )
    base.require(not base.LOCAL_PATH_RE.search(json.dumps(result)), "SPIR-V smoke result leaked a machine-local path")
    base.require(
        len(base.pretty_json(result)) <= policy["maximum_receipt_bytes"],
        "SPIR-V smoke receipt exceeds the maximum canonical receipt size",
    )
    return result


def produce(args: argparse.Namespace) -> dict:
    policy = load_policy()
    port = Path(args.port_dir).resolve()
    oracle = Path(args.oracle_dir).resolve() if args.oracle_dir else None
    denominator = base.check_denominator(port, oracle)
    dxc_source = Path(args.dxc_dir).resolve()
    dxc_receipt, dxc_closure = base.validate_build_receipt(Path(args.dxc_build_dir).resolve(), dxc_source)
    spirv_val_receipt, spirv_val_closure = validate_spirv_val_build(Path(args.spirv_val_build_dir).resolve(), dxc_source)
    output = Path(args.output_dir).resolve()
    base.prepare_output_directory(output)
    artifact_policy = base.load_policy()
    entries = []
    with (
        base.staged_dxc_compiler(dxc_closure, output) as compiler,
        staged_spirv_val(spirv_val_closure, output) as (validator, grammar_path),
        tempfile.TemporaryDirectory(prefix=".fn64-rt64-source-", dir=output) as temporary,
    ):
        snapshot = Path(temporary) / "source"
        snapshot_record = base.stage_rt64_source_snapshot(port, snapshot, denominator, artifact_policy["spirv"]["maximum_source_bytes"])
        base.require(snapshot_record["source_set_sha256"] == denominator["authority"]["port_source_set_sha256"], "private RT64 snapshot mismatch")
        source_sha_by_path = {row["path"]: row["sha256"] for row in snapshot_record["files"]}
        grammar_bytes = base.stable_file_bytes(grammar_path, artifact_policy["git_checkout_maximum_file_bytes"], "staged SPIR-V grammar")
        for expected in denominator["entries"]:
            base.require("-Vd" not in expected["flags"], f"DXC built-in SPIR-V validation disabled: {expected['id']}")
            dependency_manifest = output / expected["dependency_manifest_artifact"]
            dependency_manifest.parent.mkdir(parents=True, exist_ok=True)
            prepared = base.prepare_dxc_shader_input(compiler, snapshot, output, expected, denominator, artifact_policy)
            compiled = base.compile_dxc_shader(compiler, snapshot, output, expected, prepared, artifact_policy)
            dependency_manifest.write_bytes(base.pretty_json(prepared["active_dependencies"]))
            validation = run_spirv_val(validator, compiled["artifact_path"], expected, output, policy)
            inventory = inventory_spirv(compiled["artifact_bytes"], grammar_bytes)
            entries.append(
                {
                    **expected,
                    "source_sha256": source_sha_by_path[expected["source"]],
                    "preprocessed_sha256": base.digest_bytes(prepared["preprocessed_bytes"]),
                    "preprocessed_bytes": len(prepared["preprocessed_bytes"]),
                    "dependency_output_artifact": prepared["contract"]["dependency"]["output"],
                    "dependency_output_sha256": base.digest_bytes(prepared["dependency_bytes"]),
                    "dependency_output_bytes": len(prepared["dependency_bytes"]),
                    "dependency_manifest_sha256": base.digest_file(dependency_manifest, artifact_policy["spirv"]["maximum_dependency_manifest_bytes"]),
                    "dependency_manifest_bytes": dependency_manifest.stat().st_size,
                    "compiler_dependency_target": prepared["active_dependencies"]["depfile_target"],
                    "compiler_dependency_files": prepared["active_dependencies"]["files"],
                    "compiler_dependency_set_sha256": prepared["active_dependencies"]["dependency_set_sha256"],
                    "spirv_sha256": base.digest_bytes(compiled["artifact_bytes"]),
                    "spirv_bytes": len(compiled["artifact_bytes"]),
                    "compiler": {
                        "base_flags": expected["flags"],
                        "phase_contract": prepared["contract"],
                        "dependency_stdout_sha256": prepared["dependency_log"]["stdout_sha256"],
                        "dependency_stderr_sha256": prepared["dependency_log"]["stderr_sha256"],
                        "preprocess_stdout_sha256": prepared["preprocess_log"]["stdout_sha256"],
                        "preprocess_stderr_sha256": prepared["preprocess_log"]["stderr_sha256"],
                        "compile_stdout_sha256": compiled["compile_log"]["stdout_sha256"],
                        "compile_stderr_sha256": compiled["compile_log"]["stderr_sha256"],
                        "built_in_spirv_validation": "passed-without-Vd",
                    },
                    "spirv_val_validation": validation,
                    "semantic_inventory": inventory,
                }
            )
    artifact_set = [{"path": row["spirv_artifact"], "sha256": row["spirv_sha256"]} for row in entries]
    with staged_spirv_val(spirv_val_closure, output) as (validator, grammar_path):
        validator_record = base.tool_record(validator, ["--version"], {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"})
        grammar_record = {
            "path": policy["dxc"]["inventory_grammar_path"],
            "sha256": base.digest_file(grammar_path),
        }
    receipt = base.add_receipt_hash(
        {
            "schema": policy["receipt_schema"],
            "status": "complete",
            "orchestration_producer_sha256": base.digest_file(TOOL_PATH),
            "artifact_producer_sha256": base.digest_file(base.TOOL_PATH),
            "reference_policy_sha256": base.digest_file(POLICY_PATH),
            "artifact_policy_sha256": base.digest_file(base.POLICY_PATH),
            "denominator_sha256": denominator["denominator_sha256"],
            "source_snapshot": snapshot_record,
            "dxc_build_receipt_sha256": dxc_receipt["receipt_sha256"],
            "dxc_compiler_sha256": dxc_receipt["compiler_sha256"],
            "spirv_val_build_receipt_sha256": spirv_val_receipt["receipt_sha256"],
            "spirv_val": validator_record,
            "spirv_grammar": grammar_record,
            "required_validation": policy["required_validation"],
            "entries": entries,
            "artifact_set_sha256": base.digest_bytes(base.canonical_json(artifact_set)),
            "claim_boundary": policy["claim_boundary"],
        }
    )
    base.require(not base.LOCAL_PATH_RE.search(json.dumps(receipt)), "reference receipt leaked a machine-local path")
    (output / RECEIPT_PATH).write_bytes(base.pretty_json(receipt))
    return receipt


def require_sha(value: object, label: str) -> str:
    base.require(isinstance(value, str) and base.SHA256_RE.fullmatch(value) is not None, f"{label} is not canonical SHA-256")
    return value


def validate_reference_artifact_paths(expected: dict, row: dict) -> None:
    contract = base.dxc_phase_contract(expected)
    derived_dependency = base.dependency_output_artifact(expected)
    base.require(
        derived_dependency == contract["dependency"]["output"],
        f"DXC dependency phase contract is internally inconsistent: {expected['id']}",
    )
    base.require(
        row.get("dependency_output_artifact") == derived_dependency,
        f"reference dependency output path changed: {expected['id']}",
    )
    paths = [
        derived_dependency,
        expected["dependency_manifest_artifact"],
        expected["preprocessed_artifact"],
        expected["spirv_artifact"],
    ]
    base.require(len(set(paths)) == len(paths), f"reference artifact paths collide: {expected['id']}")


def validate_reference_receipt(
    receipt: dict,
    denominator: dict,
    artifact_dir: Path,
    dxc_receipt: dict,
    spirv_val_receipt: dict,
    validator: Path,
    grammar_path: Path,
) -> None:
    policy = load_policy()
    artifact_policy = base.load_policy()
    base.require_keys(
        receipt,
        {
            "schema",
            "status",
            "orchestration_producer_sha256",
            "artifact_producer_sha256",
            "reference_policy_sha256",
            "artifact_policy_sha256",
            "denominator_sha256",
            "source_snapshot",
            "dxc_build_receipt_sha256",
            "dxc_compiler_sha256",
            "spirv_val_build_receipt_sha256",
            "spirv_val",
            "spirv_grammar",
            "required_validation",
            "entries",
            "artifact_set_sha256",
            "claim_boundary",
            "receipt_sha256",
        },
        "reference shader receipt",
    )
    base.require(receipt["schema"] == policy["receipt_schema"] and receipt["status"] == "complete", "reference shader receipt is incomplete")
    base.validate_receipt_hash(receipt)
    base.require(receipt["orchestration_producer_sha256"] == base.digest_file(TOOL_PATH), "reference artifacts used another orchestration producer")
    base.require(receipt["artifact_producer_sha256"] == base.digest_file(base.TOOL_PATH), "reference artifacts used another artifact producer")
    base.require(receipt["reference_policy_sha256"] == base.digest_file(POLICY_PATH), "reference artifact policy changed")
    base.require(receipt["artifact_policy_sha256"] == base.digest_file(base.POLICY_PATH), "base artifact policy changed")
    base.require(receipt["denominator_sha256"] == denominator["denominator_sha256"], "reference denominator changed")
    base.require(receipt["source_snapshot"] == base.source_snapshot_record(denominator), "reference source snapshot changed")
    base.require(receipt["dxc_build_receipt_sha256"] == dxc_receipt["receipt_sha256"], "reference DXC receipt changed")
    base.require(receipt["dxc_compiler_sha256"] == dxc_receipt["compiler_sha256"], "reference DXC compiler changed")
    base.require(receipt["spirv_val_build_receipt_sha256"] == spirv_val_receipt["receipt_sha256"], "reference spirv-val receipt changed")
    validator_record = base.tool_record(validator, ["--version"], {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"})
    base.require(receipt["spirv_val"] == validator_record, "reference spirv-val identity changed")
    grammar_record = {"path": policy["dxc"]["inventory_grammar_path"], "sha256": base.digest_file(grammar_path)}
    base.require(receipt["spirv_grammar"] == grammar_record, "reference SPIR-V grammar changed")
    base.require(receipt["required_validation"] == policy["required_validation"], "reference validation denominator changed")
    base.require(receipt["claim_boundary"] == policy["claim_boundary"], "reference claim boundary changed")
    rows = receipt["entries"]
    base.require(isinstance(rows, list) and len(rows) == len(denominator["entries"]), "reference entry denominator changed")
    receipt_path = artifact_dir / RECEIPT_PATH
    receipt_info = receipt_path.lstat()
    base.require(stat.S_ISREG(receipt_info.st_mode) and not receipt_path.is_symlink(), "reference receipt is not regular")
    base.require(receipt_info.st_nlink == 1, "reference receipt is hardlinked")
    base.require(
        base.digest_file(receipt_path, policy["maximum_receipt_bytes"]) == base.digest_bytes(base.pretty_json(receipt)),
        "reference receipt bytes are not canonical",
    )
    expected_files = {RECEIPT_PATH}
    seen_objects = {(receipt_info.st_dev, receipt_info.st_ino)}
    source_by_path = {row["path"]: row for row in denominator["source_files"]}
    artifact_set = []
    grammar_bytes = base.stable_file_bytes(grammar_path, artifact_policy["git_checkout_maximum_file_bytes"], "staged SPIR-V grammar")
    for expected, row in zip(denominator["entries"], rows, strict=True):
        base.require_keys(
            row,
            set(expected)
            | {
                "source_sha256",
                "preprocessed_sha256",
                "preprocessed_bytes",
                "dependency_output_artifact",
                "dependency_output_sha256",
                "dependency_output_bytes",
                "dependency_manifest_sha256",
                "dependency_manifest_bytes",
                "compiler_dependency_target",
                "compiler_dependency_files",
                "compiler_dependency_set_sha256",
                "spirv_sha256",
                "spirv_bytes",
                "compiler",
                "spirv_val_validation",
                "semantic_inventory",
            },
            f"reference row {expected['id']}",
        )
        for key, value in expected.items():
            base.require(row[key] == value, f"reference row {expected['id']} changed {key}")
        base.require("-Vd" not in row["flags"], f"reference row disables built-in validation: {expected['id']}")
        compiler = row["compiler"]
        base.require_keys(
            compiler,
            {
                "base_flags",
                "phase_contract",
                "dependency_stdout_sha256",
                "dependency_stderr_sha256",
                "preprocess_stdout_sha256",
                "preprocess_stderr_sha256",
                "compile_stdout_sha256",
                "compile_stderr_sha256",
                "built_in_spirv_validation",
            },
            f"reference compiler row {expected['id']}",
        )
        base.require(compiler["base_flags"] == expected["flags"], f"reference base flags changed: {expected['id']}")
        base.require(compiler["phase_contract"] == base.dxc_phase_contract(expected), f"reference phase contract changed: {expected['id']}")
        validate_reference_artifact_paths(expected, row)
        base.require(compiler["built_in_spirv_validation"] == "passed-without-Vd", f"DXC built-in validation absent: {expected['id']}")
        for name in (
            "dependency_stdout_sha256",
            "dependency_stderr_sha256",
            "preprocess_stdout_sha256",
            "preprocess_stderr_sha256",
            "compile_stdout_sha256",
            "compile_stderr_sha256",
        ):
            require_sha(compiler[name], f"{expected['id']} {name}")
        base.require(row["source_sha256"] == source_by_path[expected["source"]]["port_sha256"], f"reference source identity changed: {expected['id']}")
        dependencies = row["compiler_dependency_files"]
        base.require(isinstance(dependencies, list) and dependencies, f"reference dependency closure absent: {expected['id']}")
        dependency_paths = []
        for index, dependency in enumerate(dependencies):
            base.require_keys(dependency, {"path", "sha256"}, f"reference dependency {expected['id']}[{index}]")
            path = dependency["path"]
            base.require(isinstance(path, str) and path in source_by_path, f"unknown reference dependency: {expected['id']}")
            base.require(require_sha(dependency["sha256"], f"reference dependency {path}") == source_by_path[path]["port_sha256"], f"reference dependency bytes changed: {path}")
            dependency_paths.append(path)
        base.require(dependency_paths == sorted(set(dependency_paths)), f"reference dependencies are not canonical: {expected['id']}")
        base.require(row["compiler_dependency_target"] == expected["source"], f"reference dependency target changed: {expected['id']}")
        base.require(expected["source"] in dependency_paths and set(dependency_paths) <= set(expected["dependency_files"]), f"reference dependency set escaped denominator: {expected['id']}")
        base.require(row["compiler_dependency_set_sha256"] == base.digest_bytes(base.canonical_json(dependencies)), f"reference dependency-set digest changed: {expected['id']}")
        paths = (
            (expected["preprocessed_artifact"], "preprocessed_sha256", "preprocessed_bytes", artifact_policy["spirv"]["maximum_preprocessed_bytes"]),
            (row["dependency_output_artifact"], "dependency_output_sha256", "dependency_output_bytes", artifact_policy["spirv"]["maximum_dependency_output_bytes"]),
            (expected["dependency_manifest_artifact"], "dependency_manifest_sha256", "dependency_manifest_bytes", artifact_policy["spirv"]["maximum_dependency_manifest_bytes"]),
            (expected["spirv_artifact"], "spirv_sha256", "spirv_bytes", artifact_policy["spirv"]["maximum_artifact_bytes"]),
        )
        for relative_text, digest_key, size_key, maximum in paths:
            relative = require_safe_relative(relative_text, f"reference artifact path {expected['id']}")
            expected_files.add(relative.as_posix())
            path = artifact_dir.joinpath(*relative.parts)
            info = path.lstat()
            base.require(stat.S_ISREG(info.st_mode) and not path.is_symlink(), f"reference artifact is not regular: {relative}")
            identity = (info.st_dev, info.st_ino)
            base.require(identity not in seen_objects and info.st_nlink == 1, f"reference artifact is linked or reused: {relative}")
            seen_objects.add(identity)
            base.require(info.st_size == row[size_key], f"reference artifact size changed: {relative}")
            base.require(base.digest_file(path, maximum) == require_sha(row[digest_key], f"reference artifact {relative}"), f"reference artifact digest changed: {relative}")
        dependency_output = base.stable_file_bytes(
            artifact_dir / row["dependency_output_artifact"],
            artifact_policy["spirv"]["maximum_dependency_output_bytes"],
            f"reference dependency output {expected['id']}",
        )
        base.require(base.parse_dxc_dependency_rule(dependency_output, expected) == dependency_paths, f"reference raw dependencies disagree: {expected['id']}")
        dependency_manifest = base.load_canonical_json(
            artifact_dir / expected["dependency_manifest_artifact"],
            artifact_policy["spirv"]["maximum_dependency_manifest_bytes"],
            f"reference dependency manifest {expected['id']}",
        )
        base.require(
            dependency_manifest
            == {
                "schema": base.DEPENDENCY_SCHEMA,
                "entry": expected["id"],
                "depfile_target": expected["source"],
                "files": dependencies,
                "dependency_set_sha256": row["compiler_dependency_set_sha256"],
            },
            f"reference dependency manifest changed: {expected['id']}",
        )
        spirv_path = artifact_dir / expected["spirv_artifact"]
        base.require(
            row["spirv_val_validation"] == run_spirv_val(validator, spirv_path, expected, artifact_dir, policy),
            f"spirv-val transcript changed: {expected['id']}",
        )
        artifact_bytes = base.stable_file_bytes(spirv_path, artifact_policy["spirv"]["maximum_artifact_bytes"], f"reference SPIR-V {expected['id']}")
        base.require(row["semantic_inventory"] == inventory_spirv(artifact_bytes, grammar_bytes), f"SPIR-V semantic inventory changed: {expected['id']}")
        artifact_set.append({"path": expected["spirv_artifact"], "sha256": row["spirv_sha256"]})
    base.require(receipt["artifact_set_sha256"] == base.digest_bytes(base.canonical_json(artifact_set)), "reference artifact set changed")
    actual_files = {
        path.relative_to(artifact_dir).as_posix()
        for path in artifact_dir.rglob("*")
        if path.is_file() or path.is_symlink()
    }
    base.require(actual_files == expected_files, f"reference artifact file denominator changed: {sorted(actual_files ^ expected_files)}")
    base.require(not base.LOCAL_PATH_RE.search(json.dumps(receipt)), "reference receipt contains a machine-local path")


def verify(args: argparse.Namespace) -> None:
    port = Path(args.port_dir).resolve()
    oracle = Path(args.oracle_dir).resolve() if args.oracle_dir else None
    denominator = base.check_denominator(port, oracle)
    source = Path(args.dxc_dir).resolve()
    dxc_receipt, _ = base.validate_build_receipt(Path(args.dxc_build_dir).resolve(), source)
    spirv_val_receipt, closure = validate_spirv_val_build(Path(args.spirv_val_build_dir).resolve(), source)
    artifact_dir = Path(args.artifact_dir).resolve()
    receipt = base.load_canonical_json(artifact_dir / RECEIPT_PATH, load_policy()["maximum_receipt_bytes"], "reference shader receipt")
    with tempfile.TemporaryDirectory(prefix="fn64-reference-verify-") as temporary:
        private_root = Path(temporary).resolve(strict=True)
        base.require(
            private_root != artifact_dir
            and artifact_dir not in private_root.parents
            and private_root not in artifact_dir.parents,
            "reference verifier staging overlaps the corpus tree",
        )
        private_root.chmod(0o700)
        with staged_spirv_val(closure, private_root) as (validator, grammar):
            validate_reference_receipt(receipt, denominator, artifact_dir, dxc_receipt, spirv_val_receipt, validator, grammar)


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    sub = result.add_subparsers(dest="command", required=True)
    build = sub.add_parser("build-spirv-val")
    build.add_argument("--dxc-dir", required=True)
    build.add_argument("--output-dir", required=True)
    build.add_argument("--cmake", default="cmake")
    build.add_argument("--ninja", default="ninja")
    build.add_argument("--python", default="python3")
    build.add_argument("--git", default="git")
    build.add_argument("--cc", default="cc")
    build.add_argument("--cxx", default="c++")
    verify_build = sub.add_parser("verify-spirv-val-build")
    verify_build.add_argument("--dxc-dir", required=True)
    verify_build.add_argument("--build-dir", required=True)
    smoke = sub.add_parser("smoke-spirv-val")
    smoke.add_argument("--dxc-dir", required=True)
    smoke.add_argument("--build-dir", required=True)
    smoke.add_argument("--artifact", required=True)
    smoke.add_argument("--require-shader-nonuniform", action="store_true")
    produce_parser = sub.add_parser("produce")
    produce_parser.add_argument("--port-dir", required=True)
    produce_parser.add_argument("--oracle-dir")
    produce_parser.add_argument("--dxc-dir", required=True)
    produce_parser.add_argument("--dxc-build-dir", required=True)
    produce_parser.add_argument("--spirv-val-build-dir", required=True)
    produce_parser.add_argument("--output-dir", required=True)
    verify_parser = sub.add_parser("verify")
    verify_parser.add_argument("--port-dir", required=True)
    verify_parser.add_argument("--oracle-dir")
    verify_parser.add_argument("--dxc-dir", required=True)
    verify_parser.add_argument("--dxc-build-dir", required=True)
    verify_parser.add_argument("--spirv-val-build-dir", required=True)
    verify_parser.add_argument("--artifact-dir", required=True)
    sub.add_parser("selftest")
    return result


def selftest() -> None:
    policy = load_policy()
    base.require(policy["dxc"]["inventory_grammar_sha256"] == "fc328b3a978cf6128617c679f1932717fb9a5fdbc9049c4124c2cc5f2f35cb4b", "grammar pin drift")
    grammar = base.digest_file(POLICY_PATH)
    base.require(base.SHA256_RE.fullmatch(grammar) is not None, "reference policy is not hashable")


def main() -> int:
    args = parser().parse_args()
    try:
        if args.command == "build-spirv-val":
            receipt = build_spirv_val(args)
            print(f"spirv-val source build receipt: {receipt['receipt_sha256']}")
        elif args.command == "verify-spirv-val-build":
            receipt, _ = validate_spirv_val_build(Path(args.build_dir).resolve(), Path(args.dxc_dir).resolve())
            print(f"spirv-val source build verified: {receipt['receipt_sha256']}")
        elif args.command == "smoke-spirv-val":
            print(base.pretty_json(smoke_spirv_val(args)).decode(), end="")
        elif args.command == "produce":
            receipt = produce(args)
            print(f"RT64 reference shader receipt: {receipt['receipt_sha256']}")
        elif args.command == "verify":
            verify(args)
            print("RT64 reference shader receipt and files verified")
        else:
            selftest()
            print("RT64 reference shader artifact selftest passed")
    except (base.ArtifactError, OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
