#!/usr/bin/env python3
"""Validate and render fn64's immutable RT64 port authority manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = ROOT / "docs/rt64-port-authority.json"
DOC_PATH = ROOT / "docs/RT64-PORT-AUTHORITY.md"
HEX40 = re.compile(r"[0-9a-f]{40}\Z")
HEX64 = re.compile(r"[0-9a-f]{64}\Z")
PINNED_SOURCE = re.compile(
    r'(?:PINNED_SOURCE:\s*&str\s*=|source_id\s*!=)\s*"git:([0-9a-f]{40})"'
)
SOURCE_PATH = re.compile(r'\$\{FN64_RT64_SOURCE_DIR\}/([^"\n]+\.(?:cpp|hlsl))')

EXPECTED_TOP_LEVEL = {
    "schema_version",
    "reviewed_on",
    "repository",
    "oracle",
    "port_source",
    "overlays",
    "submodules",
    "nested_submodules",
    "embedded_components",
    "excluded_or_stale",
    "system_dependencies",
    "qualification_required_for_port_source",
}
EXPECTED_SUBMODULE_PATHS = {
    "src/contrib/ddspp",
    "src/contrib/dxc",
    "src/contrib/hlslpp",
    "src/contrib/im3d",
    "src/contrib/imgui",
    "src/contrib/implot",
    "src/contrib/mupen64plus-core",
    "src/contrib/mupen64plus-win32-deps",
    "src/contrib/nativefiledialog-extended",
    "src/contrib/plume",
    "src/contrib/re-spirv",
    "src/contrib/spirv-cross",
    "src/contrib/stb",
    "src/contrib/xxHash",
    "src/contrib/zstd",
}
EXPECTED_SOURCE_GATES = {
    "src/gbi/rt64_gbi_s2dex.cpp",
    "src/shaders/RasterPS.hlsl",
    "src/render/rt64_raster_shader.cpp",
    "src/render/rt64_shader_library.cpp",
    "src/render/rt64_vi_renderer.cpp",
    "src/hle/rt64_state.cpp",
    "src/hle/rt64_interpreter.cpp",
    "src/render/rt64_raster_shader_cache.cpp",
    "src/hle/rt64_vi.cpp",
    "src/hle/rt64_present_queue.cpp",
    "src/contrib/plume/plume_metal.cpp",
}


class AuthorityError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AuthorityError(message)


def load_manifest() -> dict:
    try:
        value = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise AuthorityError(f"cannot load {MANIFEST_PATH}: {error}") from error
    require(isinstance(value, dict), "manifest root must be an object")
    return value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git(directory: Path, *arguments: str) -> str:
    try:
        return subprocess.run(
            ["git", "-C", str(directory), *arguments],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        raise AuthorityError(
            f"git {' '.join(arguments)} failed in {directory}: {detail.strip()}"
        ) from error


def validate_manifest(manifest: dict) -> None:
    require(set(manifest) == EXPECTED_TOP_LEVEL, "manifest top-level keys changed")
    require(manifest["schema_version"] == 1, "unsupported authority schema")
    require(manifest["repository"] == "https://github.com/rt64/rt64", "unexpected repository")

    oracle = manifest["oracle"]
    port = manifest["port_source"]
    require(HEX40.fullmatch(oracle["commit"]) is not None, "oracle commit must be lowercase SHA-1")
    require(oracle["source_id"] == f"git:{oracle['commit']}", "oracle source_id drift")
    require(oracle["status"] == "active-gated", "oracle must remain explicitly gated")
    require(oracle["license"] == "MIT", "RT64 root license must remain MIT")
    require(HEX64.fullmatch(oracle["license_sha256"]) is not None, "bad root license digest")
    require(HEX64.fullmatch(oracle["gitmodules_sha256"]) is not None, "bad .gitmodules digest")
    require(HEX40.fullmatch(port["commit"]) is not None, "port source commit must be lowercase SHA-1")
    require(port["commit"] != oracle["commit"], "port source must not masquerade as the gated oracle")
    require(port["status"] == "reviewed-not-runtime-qualified", "port source status drift")
    require(
        port["disposition"] == "accepted-port-input-retain-old-oracle",
        "port source disposition drift",
    )
    require(port["commits_ahead_of_oracle"] == 9, "reviewed upstream delta count drift")

    overlays = manifest["overlays"]
    require(overlays["default_id"].startswith("fn64:"), "default overlay id must be namespaced")
    require(
        overlays["hfr_id"] == overlays["default_id"] + "+hfr-post-present-call:v1",
        "HFR overlay id must extend the default identity exactly",
    )
    gates = overlays["source_gates"]
    gate_paths = [gate["path"] for gate in gates]
    require(len(gate_paths) == len(set(gate_paths)), "duplicate overlay source gate")
    require(set(gate_paths) == EXPECTED_SOURCE_GATES, "overlay source-gate denominator changed")
    for gate in gates:
        require(HEX64.fullmatch(gate["sha256"]) is not None, f"{gate['path']}: bad SHA-256")
        require(gate["mechanisms"], f"{gate['path']}: no named mechanism")
        require(re.fullmatch(r"M(?:[3-9]|10)", gate["port_milestone"]) is not None, f"{gate['path']}: bad milestone")
        for mechanism in gate["mechanisms"]:
            if mechanism == "plume-metal-lifetime:v1":
                continue
            expected_id = overlays["hfr_id"] if mechanism == "hfr-post-present-call:v1" else overlays["default_id"]
            require(mechanism in expected_id, f"{gate['path']}: mechanism absent from overlay identity")

    submodules = manifest["submodules"]
    paths = [item["path"] for item in submodules]
    require(len(paths) == len(set(paths)), "duplicate direct submodule")
    require(set(paths) == EXPECTED_SUBMODULE_PATHS, "direct submodule denominator changed")
    for item in submodules:
        require(HEX40.fullmatch(item["oracle_revision"]) is not None, f"{item['path']}: bad oracle revision")
        require(HEX40.fullmatch(item["port_revision"]) is not None, f"{item['path']}: bad port revision")
        require(item["admission"] in {"allowed", "excluded", "blocked-windows"}, f"{item['path']}: bad admission")
        if item["license_path"] is None:
            require(item["license_sha256"] is None, f"{item['path']}: digest without license path")
            require(item["license"] == "NOASSERTION", f"{item['path']}: missing license must be NOASSERTION")
        else:
            require(HEX64.fullmatch(item["license_sha256"]) is not None, f"{item['path']}: bad license digest")

    nested_paths: set[str] = set()
    for item in manifest["nested_submodules"]:
        require(item["path"] not in nested_paths, f"duplicate nested submodule {item['path']}")
        nested_paths.add(item["path"])
        require(HEX40.fullmatch(item["revision"]) is not None, f"{item['path']}: bad revision")
        require(HEX64.fullmatch(item["license_sha256"]) is not None, f"{item['path']}: bad license digest")
        require(item["admission"] == "allowed", f"{item['path']}: nested dependency is not admitted")

    embedded_paths = [item["path"] for item in manifest["embedded_components"]]
    require(len(embedded_paths) == len(set(embedded_paths)), "duplicate embedded component")
    excluded_paths = {item["path"] for item in manifest["excluded_or_stale"]}
    require("src/tools/texture_hasher" in excluded_paths, "GPL-derived texture hasher exclusion missing")
    require("src/contrib/xess" in excluded_paths, "stale XeSS declaration not recorded")
    require(manifest["qualification_required_for_port_source"], "candidate qualification list is empty")


def validate_local_consumers(manifest: dict) -> None:
    oracle_commit = manifest["oracle"]["commit"]
    feature_inventory = json.loads((ROOT / "docs/rt64-public-feature-inventory.json").read_text())
    macos_inventory = json.loads((ROOT / "docs/rt64-macos-certification.json").read_text())
    require(feature_inventory["upstream"]["commit"] == oracle_commit, "feature inventory pin differs from authority")
    require(macos_inventory["source"]["rt64_commit"] == oracle_commit, "macOS certification pin differs from authority")
    require(macos_inventory["source"]["source_id"] == f"git:{oracle_commit}", "macOS source id differs from authority")

    pinned_roots = [
        ROOT / "crates/fn64-render-rt64/examples",
        ROOT / "crates/fn64-render-rt64/src",
        ROOT / "crates/fn64-certification/examples",
    ]
    observed: set[str] = set()
    for source_root in pinned_roots:
        for source in source_root.rglob("*.rs"):
            observed.update(PINNED_SOURCE.findall(source.read_text(encoding="utf-8")))
    require(observed == {oracle_commit}, f"source-pinned Rust evidence disagrees: {sorted(observed)}")

    build_source = (ROOT / "crates/fn64-render-rt64/build.rs").read_text(encoding="utf-8")
    require("RT64_AUTHORITY_MANIFEST" in build_source, "build.rs does not consume the authority manifest")
    require(
        'env::var_os("FN64_RT64_SOURCE_ID")' not in build_source,
        "build.rs still accepts an unconstrained declared source identity",
    )
    cmake_source = (ROOT / "crates/fn64-render-rt64/ffi/CMakeLists.txt").read_text(encoding="utf-8")
    cmake_gate_paths = set(SOURCE_PATH.findall(cmake_source))
    require(cmake_gate_paths == EXPECTED_SOURCE_GATES, "CMake overlay source denominator differs from manifest")
    for gate in manifest["overlays"]["source_gates"]:
        require(gate["sha256"] in cmake_source, f"CMake does not fail closed on {gate['path']}")


def tree_gitlink(tree: Path, revision: str, component_path: str) -> str:
    line = git(tree, "ls-tree", revision, component_path)
    fields = line.split()
    require(len(fields) >= 3 and fields[0] == "160000" and fields[1] == "commit", f"{component_path}: not a gitlink")
    return fields[2]


def validate_oracle_tree(manifest: dict, tree: Path) -> None:
    require(tree.is_dir(), f"oracle directory does not exist: {tree}")
    oracle = manifest["oracle"]
    require(git(tree, "rev-parse", "HEAD") == oracle["commit"], "oracle checkout is at the wrong commit")
    require(not git(tree, "status", "--porcelain", "--", "."), "oracle checkout or submodule is dirty")
    require(sha256_file(tree / oracle["license_path"]) == oracle["license_sha256"], "RT64 LICENSE digest mismatch")
    require(sha256_file(tree / ".gitmodules") == oracle["gitmodules_sha256"], "RT64 .gitmodules digest mismatch")
    for gate in manifest["overlays"]["source_gates"]:
        require(sha256_file(tree / gate["path"]) == gate["sha256"], f"{gate['path']}: source gate mismatch")
    for item in manifest["submodules"]:
        require(tree_gitlink(tree, oracle["commit"], item["path"]) == item["oracle_revision"], f"{item['path']}: oracle gitlink mismatch")
        if item["license_path"] is not None:
            require(sha256_file(tree / item["license_path"]) == item["license_sha256"], f"{item['path']}: license digest mismatch")
    for item in manifest["nested_submodules"]:
        component = tree / item["path"]
        require(component.is_dir(), f"nested submodule is not initialized: {item['path']}")
        require(git(component, "rev-parse", "HEAD") == item["revision"], f"{item['path']}: nested revision mismatch")
        require(sha256_file(tree / item["license_path"]) == item["license_sha256"], f"{item['path']}: license digest mismatch")


def validate_port_tree(manifest: dict, tree: Path) -> None:
    require(tree.is_dir(), f"port-source directory does not exist: {tree}")
    port = manifest["port_source"]
    oracle = manifest["oracle"]
    require(git(tree, "rev-parse", "HEAD") == port["commit"], "port-source checkout is at the wrong commit")
    require(not git(tree, "status", "--porcelain", "--", "."), "port-source checkout is dirty")
    require(sha256_file(tree / oracle["license_path"]) == oracle["license_sha256"], "port-source LICENSE changed")
    require(sha256_file(tree / ".gitmodules") == oracle["gitmodules_sha256"], "port-source .gitmodules changed")
    ahead = int(git(tree, "rev-list", "--count", f"{oracle['commit']}..{port['commit']}"))
    require(ahead == port["commits_ahead_of_oracle"], "port-source delta count mismatch")
    for item in manifest["submodules"]:
        require(tree_gitlink(tree, port["commit"], item["path"]) == item["port_revision"], f"{item['path']}: port gitlink mismatch")


def render_document(manifest: dict) -> str:
    oracle = manifest["oracle"]
    port = manifest["port_source"]
    lines = [
        "# RT64 port authority",
        "",
        "<!-- Generated by tools/check_rt64_port_authority.py from docs/rt64-port-authority.json. -->",
        "",
        "This is the machine-checked source, overlay, and license boundary for the",
        "RT64-to-Rust program. Edit the JSON manifest, then regenerate this file;",
        "do not edit this report directly.",
        "",
        "## Decision",
        "",
        f"- Reviewed: `{manifest['reviewed_on']}`.",
        f"- Executable oracle: [`{oracle['commit'][:7]}`]({manifest['repository']}/commit/{oracle['commit']}) (`{oracle['status']}`).",
        f"- Rust-port source: [`{port['commit'][:7]}`]({manifest['repository']}/commit/{port['commit']}) (`{port['status']}`).",
        f"- Disposition: `{port['disposition']}`.",
        "",
        port["reason"],
        "",
        "The port source is accepted design/behavior input, not an executable",
        "replacement for the source-bound RT64 receipts. Historical evidence remains",
        "attached to the old oracle until every qualification item below passes.",
        "",
        "## Fn64 source overlays",
        "",
        f"Default identity: `{manifest['overlays']['default_id']}`.",
        "",
        "| upstream source | SHA-256 | fn64 mechanism | Rust milestone |",
        "|---|---|---|---|",
    ]
    for gate in manifest["overlays"]["source_gates"]:
        lines.append(
            f"| `{gate['path']}` | `{gate['sha256']}` | "
            f"`{', '.join(gate['mechanisms'])}` | `{gate['port_milestone']}` |"
        )
    lines.extend([
        "",
        "## Direct submodule closure",
        "",
        "| path | executable oracle | port source | license | admission |",
        "|---|---|---|---|---|",
    ])
    for item in manifest["submodules"]:
        lines.append(
            f"| `{item['path']}` | `{item['oracle_revision'][:7]}` | "
            f"`{item['port_revision'][:7]}` | `{item['license']}` | `{item['admission']}` |"
        )
    lines.extend([
        "",
        "`allowed` means admissible for the reviewed source closure, not proof that",
        "every platform links the component. `excluded` must remain outside the fn64",
        "target. `blocked-windows` is an explicit distribution-certification blocker.",
        "",
        "## Nested and embedded closure",
        "",
        "| path | identity | license | admission |",
        "|---|---|---|---|",
    ])
    for item in manifest["nested_submodules"]:
        lines.append(
            f"| `{item['path']}` | `{item['revision'][:7]}` | `{item['license']}` | `{item['admission']}` |"
        )
    for item in manifest["embedded_components"]:
        lines.append(
            f"| `{item['path']}` | `{item['identity']}` | `{item['license']}` | `{item['admission']}` |"
        )
    lines.extend([
        "",
        "## Exclusions and blockers",
        "",
    ])
    for item in manifest["excluded_or_stale"]:
        lines.append(f"- `{item['path']}`: {item['reason']}")
    for item in manifest["submodules"] + manifest["embedded_components"]:
        if item["admission"] == "blocked-windows":
            lines.append(f"- `{item['path']}`: {item.get('note', 'Windows certification is blocked.')}")
    lines.extend([
        "- macOS/Linux system SDL2 and Linux X11/Xrandr versions are build-environment",
        "  evidence owned by M0.3; they are not silently treated as pinned here.",
        "",
        "## Port-source qualification still required",
        "",
    ])
    for item in manifest["qualification_required_for_port_source"]:
        lines.append(f"- [ ] {item}")
    lines.extend([
        "",
        "## Validation",
        "",
        "Run:",
        "",
        "```sh",
        "tools/check_rt64_port_authority.py",
        "tools/check_rt64_port_authority.py --rt64-dir /absolute/path/to/clean/oracle",
        "tools/check_rt64_port_authority.py --port-dir /absolute/path/to/clean/candidate",
        "```",
        "",
        "The ordinary check validates schema, generated-report drift, local source-pin",
        "consumers, CMake overlay coverage, and source digests. The checkout checks",
        "add root identity, cleanliness, gitlinks, nested revisions, and license bytes.",
        "",
    ])
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--write-doc", action="store_true")
    parser.add_argument("--rt64-dir", type=Path)
    parser.add_argument("--port-dir", type=Path)
    arguments = parser.parse_args()
    try:
        manifest = load_manifest()
        validate_manifest(manifest)
        validate_local_consumers(manifest)
        expected_doc = render_document(manifest)
        if arguments.write_doc:
            DOC_PATH.write_text(expected_doc, encoding="utf-8")
        else:
            require(DOC_PATH.is_file(), f"generated report is missing: {DOC_PATH}")
            require(DOC_PATH.read_text(encoding="utf-8") == expected_doc, "generated authority report is stale; run with --write-doc")
        if arguments.rt64_dir is not None:
            validate_oracle_tree(manifest, arguments.rt64_dir.resolve())
        if arguments.port_dir is not None:
            validate_port_tree(manifest, arguments.port_dir.resolve())
    except AuthorityError as error:
        print(f"rt64-port-authority: {error}", file=sys.stderr)
        return 1
    print("rt64-port-authority: clean")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
