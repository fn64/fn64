#!/usr/bin/env python3
"""Build and validate fn64's dual-pin RT64-to-Rust source/task denominator.

Only path names, digests, include edges, and non-exhaustive navigation hints
leave the admitted MIT RT64 checkouts.  Implementation text is never emitted.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parent.parent
AUTHORITY_PATH = ROOT / "docs/rt64-port-authority.json"
DEFAULT_JSON = ROOT / "docs/rt64-port-inventory.json"
DEFAULT_DOC = ROOT / "docs/RT64-PORT-INVENTORY.md"
SCHEMA = "fn64.rt64-port-inventory.v2"
EXPECTED_SOURCE_SET_SHA256 = "86704d407a71722233e71938b8517d647b38b6d2ff71d1702cc7c5e5c0232c8b"
SOURCE_SELECTIONS = ("oracle", "port")
SOURCE_PREFIXES = (
    "include",
    "src/apple",
    "src/common",
    "src/gbi",
    "src/gui",
    "src/hle",
    "src/imgui",
    "src/preset",
    "src/render",
    "src/rhi",
    "src/shaders",
    "src/shared",
)
SUFFIXES = {".cpp", ".h", ".hpp", ".mm", ".hlsl", ".hlsli"}
EXCLUDED_PREFIXES = ("src/contrib/", "src/tools/")
EXCLUSION_RECORDS = [
    {
        "path": "src/tools/texture_hasher",
        "reason": "Separate GPL-derived Rice-generation tool; its GLIDEN64-LICENSE lineage is not Rust-port authority.",
    },
    {
        "path": "src/contrib/mupen64plus-core",
        "reason": "GPL runtime implementation excluded from the fn64 target and from port authority.",
    },
    {
        "path": "m2c",
        "reason": "Excluded conversion tool under AGENTS.md; it is neither invoked nor inspected by this inventory.",
    },
]
INCLUDE = re.compile(r'^\s*#\s*include\s+"([^"\\]+)"', re.MULTILINE)
DECLARATION_HINT = re.compile(
    r"\b(class|struct|enum(?:\s+class)?)\s+(?:alignas\s*\([^)]*\)\s*)?([A-Za-z_]\w*)"
)
FUNCTION_DEFINITION_HINT = re.compile(
    r"(?:^|[;}])\s*(?:[\w:<>,~*&\[\]\s]+?)\b"
    r"([A-Za-z_]\w*(?:::[A-Za-z_]\w*)?)\s*\([^;{}]*\)"
    r"\s*(?:const\s*)?(?:noexcept\s*)?(?:\{|:\s*[^;{]+\{)",
    re.MULTILINE,
)
NON_FUNCTION_HINTS = {
    "alignas", "catch", "constexpr", "defined", "for", "if", "numthreads",
    "return", "sizeof", "switch", "while",
}
MILESTONES = {"M1", "M3", "M4", "M5", "M6", "M8", "M10", "M11", "M12"}
TASK_KEYS = {
    "id", "outcome", "authority", "owner_lane", "recommended_profile",
    "writable_paths", "non_goals", "baseline_command", "exit_gate",
    "evidence_state", "claim_status",
}
LOCAL_PATH = re.compile(r"(?:/Users/|/home/|[A-Za-z]:\\\\)")
AUDITED_M8_PATHS = frozenset({
    "include/rt64_extended_gbi.h",
    "src/common/rt64_common.cpp",
    "src/common/rt64_common.h",
    "src/common/rt64_emulator_configuration.cpp",
    "src/common/rt64_emulator_configuration.h",
    "src/common/rt64_enhancement_configuration.cpp",
    "src/common/rt64_enhancement_configuration.h",
    "src/common/rt64_filesystem.h",
    "src/common/rt64_filesystem_directory.h",
    "src/common/rt64_filesystem_zip.cpp",
    "src/common/rt64_filesystem_zip.h",
    "src/common/rt64_hlslpp.h",
    "src/common/rt64_load_types.cpp",
    "src/common/rt64_load_types.h",
    "src/common/rt64_mapped_file.cpp",
    "src/common/rt64_mapped_file.h",
    "src/common/rt64_math.cpp",
    "src/common/rt64_math.h",
    "src/common/rt64_plume.h",
    "src/common/rt64_replacement_database.cpp",
    "src/common/rt64_replacement_database.h",
    "src/common/rt64_sommelier.h",
    "src/common/rt64_thread.cpp",
    "src/common/rt64_thread.h",
    "src/common/rt64_tmem_hasher.h",
    "src/common/rt64_user_configuration.cpp",
    "src/common/rt64_user_configuration.h",
    "src/common/rt64_user_paths.cpp",
    "src/common/rt64_user_paths.h",
    "src/gbi/rt64_gbi_extended.cpp",
    "src/gbi/rt64_gbi_extended.h",
    "src/gui/rt64_camera_controller.cpp",
    "src/gui/rt64_camera_controller.h",
    "src/gui/rt64_debugger_inspector.cpp",
    "src/gui/rt64_debugger_inspector.h",
    "src/gui/rt64_file_dialog.cpp",
    "src/gui/rt64_file_dialog.h",
    "src/gui/rt64_inspector.cpp",
    "src/gui/rt64_inspector.h",
    "src/hle/rt64_application.cpp",
    "src/hle/rt64_application.h",
    "src/hle/rt64_color_converter.cpp",
    "src/hle/rt64_color_converter.h",
    "src/hle/rt64_command_warning.cpp",
    "src/hle/rt64_command_warning.h",
    "src/hle/rt64_draw_call.cpp",
    "src/hle/rt64_draw_call.h",
    "src/hle/rt64_game_call.h",
    "src/hle/rt64_game_configuration.h",
    "src/hle/rt64_game_frame.cpp",
    "src/hle/rt64_game_frame.h",
    "src/hle/rt64_light_manager.cpp",
    "src/hle/rt64_light_manager.h",
    "src/hle/rt64_present_queue.cpp",
    "src/hle/rt64_present_queue.h",
    "src/hle/rt64_projection.cpp",
    "src/hle/rt64_projection.h",
    "src/hle/rt64_rigid_body.cpp",
    "src/hle/rt64_rigid_body.h",
    "src/hle/rt64_shared_queue_resources.h",
    "src/hle/rt64_transform_group.h",
    "src/hle/rt64_vi.cpp",
    "src/hle/rt64_vi.h",
    "src/imgui/imgui_impl_sdl2_custom.cpp",
    "src/imgui/imgui_impl_sdl2_custom.h",
    "src/preset/rt64_preset.cpp",
    "src/preset/rt64_preset.h",
    "src/preset/rt64_preset_draw_call.cpp",
    "src/preset/rt64_preset_draw_call.h",
    "src/preset/rt64_preset_inspector.h",
    "src/preset/rt64_preset_light.cpp",
    "src/preset/rt64_preset_light.h",
    "src/preset/rt64_preset_material.cpp",
    "src/preset/rt64_preset_material.h",
    "src/preset/rt64_preset_scene.cpp",
    "src/preset/rt64_preset_scene.h",
    "src/render/rt64_geometry_mode.cpp",
    "src/render/rt64_geometry_mode.h",
    "src/render/rt64_look_at_processor.cpp",
    "src/render/rt64_look_at_processor.h",
    "src/render/rt64_projection_processor.cpp",
    "src/render/rt64_projection_processor.h",
    "src/render/rt64_sampler_library.h",
    "src/render/rt64_shader_common.cpp",
    "src/render/rt64_shader_common.h",
    "src/render/rt64_shader_library.h",
    "src/render/rt64_transform_processor.cpp",
    "src/render/rt64_transform_processor.h",
    "src/render/rt64_vertex_processor.cpp",
    "src/render/rt64_vertex_processor.h",
    "src/shaders/Color.hlsli",
    "src/shaders/ComposePS.hlsl",
    "src/shaders/Constants.hlsli",
    "src/shaders/DebugPS.hlsl",
    "src/shaders/Formats.hlsli",
    "src/shaders/FullScreenVS.hlsl",
    "src/shaders/IdleCS.hlsl",
    "src/shaders/Im3DCommon.hlsli",
    "src/shaders/Im3DPS.hlsl",
    "src/shaders/Im3DVS.hlsl",
    "src/shaders/Math.hlsli",
})


class InventoryError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise InventoryError(message)


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def digest(path: Path) -> str:
    return digest_bytes(path.read_bytes())


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
        raise InventoryError(f"git {' '.join(arguments)} failed: {detail.strip()}") from error


def load_authority() -> dict:
    try:
        authority = json.loads(AUTHORITY_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise InventoryError(f"cannot load authority manifest: {error}") from error
    require(authority.get("schema_version") == 1, "unsupported RT64 authority schema")
    require(authority.get("repository") == "https://github.com/rt64/rt64", "unexpected authority repository")
    return authority


def source_identity(authority: dict, selection: str) -> dict:
    if selection == "oracle":
        item = authority["oracle"]
        source_id = item["source_id"]
    else:
        item = authority["port_source"]
        source_id = f"git:{item['commit']}"
    return {
        "commit": item["commit"],
        "source_id": source_id,
        "authority_status": item["status"],
    }


def validate_tree(tree: Path, authority: dict, selection: str) -> None:
    identity = source_identity(authority, selection)
    require(tree.is_dir(), f"{selection} RT64 checkout does not exist")
    require(git(tree, "rev-parse", "HEAD") == identity["commit"], f"{selection} checkout is at the wrong authority pin")
    require(
        not git(tree, "status", "--porcelain", "--untracked-files=all", "--ignore-submodules=none"),
        f"{selection} checkout is dirty",
    )
    require((tree / "LICENSE").is_file(), f"{selection} RT64 checkout lacks LICENSE")
    require(digest(tree / "LICENSE") == authority["oracle"]["license_sha256"], f"{selection} RT64 LICENSE digest mismatch")
    require(digest(tree / ".gitmodules") == authority["oracle"]["gitmodules_sha256"], f"{selection} RT64 .gitmodules digest mismatch")
    plume = next(item for item in authority["submodules"] if item["path"] == "src/contrib/plume")
    plume_tree = tree / plume["path"]
    require(plume_tree.is_dir(), f"{selection} Plume submodule is not initialized")
    require(git(plume_tree, "rev-parse", "HEAD") == plume[f"{selection}_revision"], f"{selection} Plume checkout is at the wrong pin")


def allowed_authority_exceptions(authority: dict) -> set[str]:
    return {
        gate["path"]
        for gate in authority["overlays"]["source_gates"]
        if gate["path"].startswith(EXCLUDED_PREFIXES)
    }


def authority_locator(authority: dict, selection: str, relative: str) -> str:
    if relative.startswith("src/contrib/plume/"):
        plume = next(item for item in authority["submodules"] if item["path"] == "src/contrib/plume")
        nested = relative.removeprefix("src/contrib/plume/")
        return f"git:{plume[f'{selection}_revision']}:{nested}"
    return f"git:{source_identity(authority, selection)['commit']}:{relative}"


def source_paths(tree: Path, authority: dict) -> list[str]:
    tracked = git(tree, "ls-files").splitlines()
    result = {
        path
        for path in tracked
        if path.startswith(tuple(prefix + "/" for prefix in SOURCE_PREFIXES))
        and PurePosixPath(path).suffix in SUFFIXES
    }
    exceptions = allowed_authority_exceptions(authority)
    for path in exceptions:
        require((tree / path).is_file(), f"authority-gated source is missing: {path}")
        require(PurePosixPath(path).suffix in SUFFIXES, f"authority-gated source has unsupported suffix: {path}")
        result.add(path)
    require(result, "no admitted RT64 host or shader source files found")
    return sorted(result)


def strip_comments(text: str) -> str:
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.DOTALL)
    return re.sub(r"//[^\n]*", "", text)


def candidate_hints(text: str) -> list[dict[str, str]]:
    """Return non-exhaustive navigation hints, never a symbol denominator."""
    clean = strip_comments(text)
    observed: set[tuple[str, str]] = set()
    for _kind, name in DECLARATION_HINT.findall(clean):
        observed.add(("type-declaration", name))
    for name in FUNCTION_DEFINITION_HINT.findall(clean):
        bare = name.rsplit("::", 1)[-1]
        if bare not in NON_FUNCTION_HINTS:
            observed.add(("function-definition", name))
    return [
        {"kind": kind, "name": name}
        for kind, name in sorted(observed, key=lambda item: (item[1], item[0]))
    ]


def dependency_paths(tree: Path, relative: str, known: set[str]) -> list[str]:
    path = tree / relative
    text = path.read_text(encoding="utf-8", errors="replace")
    result: set[str] = set()
    for include in INCLUDE.findall(text):
        candidates = (path.parent / include, tree / include, tree / "src" / include)
        for candidate in candidates:
            try:
                resolved = candidate.resolve().relative_to(tree.resolve()).as_posix()
            except ValueError:
                continue
            if resolved in known:
                result.add(resolved)
                break
    return sorted(result)


def route_for(relative: str, gates: dict[str, dict]) -> tuple[str, str, str, str, str]:
    """Return the audited primary milestone, workstream, lane, profile, state.

    Rules are deliberately closed: a new file that does not match a named
    family traps instead of falling through to feature parity.
    """
    if relative in gates:
        milestone = gates[relative]["port_milestone"]
        lane = "authority-evidence" if milestone != "M10" else "gpu-render"
        profile = "M/medium" if lane == "authority-evidence" else "P/high"
        return milestone, "authority-overlay", lane, profile, "authority-gated"

    lower = relative.lower()
    name = PurePosixPath(lower).name
    if relative == "include/rt64_extended_gbi.h" or "gbi_extended" in lower:
        return "M8", "feature-parity", "semantic-frontend", "I/high", "not-started"
    if any(token in lower for token in ("raytracing", "globalhit", "lights.hlsli")):
        return "M12", "ray-path-tracing", "gpu-render", "I/high", "not-started"
    if lower.startswith("src/gbi/") or any(token in lower for token in ("/rt64_rsp", "microcode", "rspmodify", "rspprocess", "rspsmooth", "rspvertextest", "rspworld")):
        return "M5", "gbi-deferred-rsp", "semantic-frontend", "I/high", "not-started"
    if lower.startswith("src/shared/") and any(token in name for token in ("f3d", "point_light", "rsp_")):
        return "M5", "gbi-deferred-rsp", "semantic-frontend", "I/high", "not-started"
    if lower.startswith("src/shaders/fb") or any(token in lower for token in ("framebuffer", "/rt64_rdp", "raster", "texture", "tile_processor", "native_target", "render_target", "videointerface", "video_interface", "vi_renderer", "renderparams", "postblend", "rtcopy", "depth.hlsli", "random.hlsli", "bluenoise.hlsli", "background.hlsli", "library.hlsli")):
        return "M4", "rdp-framebuffer", "gpu-render", "I/high", "not-started"
    if lower.startswith("src/shared/") and any(token in name for token in ("blender", "color_combiner", "fb_", "other_mode", "rdp_", "render_params", "render_indices", "render_flags", "gpu_tile", "interleaved")):
        return "M4", "rdp-framebuffer", "gpu-render", "I/high", "not-started"
    if any(token in lower for token in ("state", "workload", "present.h", "interpreter.h")):
        return "M3", "raw-dpc", "semantic-frontend", "F/xhigh", "not-started"
    if lower.startswith("src/shared/") and any(token in name for token in ("extra_params", "frame_params", "hlsl")):
        return "M1", "semantic-ir", "semantic-frontend", "F/xhigh", "not-started"
    if any(token in lower for token in ("timer", "buffer_uploader", "descriptor_sets", "render_worker", "shader_compiler")):
        return "M6", "performance-spine", "integration-performance", "I/high", "not-started"
    if lower.startswith(("src/apple/", "src/rhi/")) or any(token in lower for token in ("application_window", "dynamic_libraries", "optimus")):
        return "M10", "platform-cutover", "gpu-render", "P/high", "not-started"
    if any(token in lower for token in ("upscaler", "postprocess", "histogram", "bicubic", "boxfilter", "gaussian", "luminance")):
        return "M11", "modernization", "gpu-render", "I/high", "not-started"
    if relative in AUDITED_M8_PATHS:
        return "M8", "feature-parity", "semantic-frontend", "I/high", "not-started"
    raise InventoryError(f"unrouted admitted RT64 source: {relative}")


def card_id(relative: str, milestone: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", relative.lower()).strip("-")
    return f"rt64-port-{milestone.lower()}-{slug}"


def rust_destination(relative: str, milestone: str) -> str:
    path = PurePosixPath(relative)
    suffix = path.suffix.lstrip(".").lower()
    stem = re.sub(r"[^a-z0-9]+", "_", path.stem.lower()).strip("_")
    if path.suffix in {".hlsl", ".hlsli"}:
        return f"crates/fn64-render-wgpu/src/shaders/{stem}_{suffix}.wgsl"
    area = {
        "M1": "ported_ir", "M3": "raw_dpc", "M4": "rdp", "M5": "gbi",
        "M6": "performance", "M8": "features", "M10": "platform",
        "M11": "modernization", "M12": "tracing",
    }[milestone]
    crate = "fn64-render-ir" if milestone == "M1" else "fn64-render-wgpu"
    return f"crates/{crate}/src/{area}/{stem}_{suffix}.rs"


def snapshot(tree: Path, relative: str, known: set[str]) -> dict:
    data = (tree / relative).read_bytes()
    text = data.decode("utf-8", errors="replace")
    return {
        "sha256": digest_bytes(data),
        "lines": len(text.splitlines()),
        "candidate_hints": candidate_hints(text),
        "dependencies": dependency_paths(tree, relative, known),
    }


def delta_kind(oracle: dict | None, port: dict | None) -> str:
    if oracle is None:
        return "added"
    if port is None:
        return "removed"
    return "unchanged" if oracle["sha256"] == port["sha256"] else "modified"


def source_set_digest(files: list[dict]) -> str:
    rows = [
        {
            "path": item["path"],
            "oracle_sha256": None if item["sources"]["oracle"] is None else item["sources"]["oracle"]["sha256"],
            "port_sha256": None if item["sources"]["port"] is None else item["sources"]["port"]["sha256"],
            "port_delta": item["port_delta"],
        }
        for item in files
    ]
    return digest_bytes((json.dumps(rows, separators=(",", ":"), sort_keys=True) + "\n").encode())


def build_inventory(oracle_tree: Path, port_tree: Path, authority: dict) -> dict:
    validate_tree(oracle_tree, authority, "oracle")
    validate_tree(port_tree, authority, "port")
    gates = {item["path"]: item for item in authority["overlays"]["source_gates"]}
    paths_by_source = {
        "oracle": source_paths(oracle_tree, authority),
        "port": source_paths(port_tree, authority),
    }
    known_by_source = {name: set(paths) for name, paths in paths_by_source.items()}
    all_paths = sorted(known_by_source["oracle"] | known_by_source["port"])
    files: list[dict] = []
    trees = {"oracle": oracle_tree, "port": port_tree}
    for relative in all_paths:
        milestone, workstream, owner, profile, port_state = route_for(relative, gates)
        sources = {
            name: snapshot(trees[name], relative, known_by_source[name]) if relative in known_by_source[name] else None
            for name in SOURCE_SELECTIONS
        }
        destination = rust_destination(relative, milestone)
        item = {
            "path": relative,
            "sources": sources,
            "port_delta": delta_kind(sources["oracle"], sources["port"]),
            "milestone": milestone,
            "workstream": workstream,
            "port_state": port_state,
            "evidence_state": "source-digests-verified",
            "task_card": {
                "id": card_id(relative, milestone),
                "outcome": f"Port the admitted behavior represented by {relative} into an owned Rust module without widening behavior claims.",
                "authority": {
                    "port_source": authority_locator(authority, "port", relative),
                    "comparison_oracle": authority_locator(authority, "oracle", relative),
                    "plan": "docs/RENDER-WGPU-PORT-PLAN.md",
                },
                "owner_lane": owner,
                "recommended_profile": profile,
                "writable_paths": [destination],
                "non_goals": [
                    "Do not edit, vendor, or transliterate the RT64 C++ source.",
                    "Do not claim parity from source translation or inventory status.",
                ],
                "baseline_command": "python3 tools/rt64_port_inventory.py --check --oracle-dir <clean-oracle> --port-dir <clean-port-source>",
                "exit_gate": f"The {milestone} behavior fixture for {relative} passes its declared differential and required 10/20-run reliability bar.",
                "evidence_state": "not-run",
                "claim_status": "candidate-observation",
            },
        }
        if relative in gates:
            item["authority_gate"] = {
                "mechanisms": gates[relative]["mechanisms"],
                "oracle_sha256": gates[relative]["sha256"],
            }
            require(sources["oracle"] is not None, f"authority gate absent from oracle: {relative}")
            require(sources["oracle"]["sha256"] == gates[relative]["sha256"], f"authority source-gate digest mismatch: {relative}")
        files.append(item)
    require(set(gates) <= set(all_paths), f"authority gates missing from inventory: {sorted(set(gates) - set(all_paths))}")
    counts = {kind: sum(item["port_delta"] == kind for item in files) for kind in ("added", "removed", "modified", "unchanged")}
    value = {
        "schema": SCHEMA,
        "generated_by": "tools/rt64_port_inventory.py",
        "authority_manifest": "docs/rt64-port-authority.json",
        "sources": {
            "repository": authority["repository"],
            "oracle": source_identity(authority, "oracle"),
            "port": source_identity(authority, "port"),
            "primary_port_input": "port",
        },
        "scope": {
            "included_prefixes": list(SOURCE_PREFIXES),
            "authority_gated_exceptions": sorted(allowed_authority_exceptions(authority)),
            "excluded_prefixes": list(EXCLUDED_PREFIXES),
            "exclusions": EXCLUSION_RECORDS,
            "file_extensions": sorted(SUFFIXES),
            "note": "Project-owned RT64 host/shader source plus explicitly authority-gated overlay files; all other contrib/tools trees remain excluded.",
        },
        "port_delta_counts": counts,
        "source_set_sha256": "",
        "files": files,
    }
    value["source_set_sha256"] = source_set_digest(files)
    return value


def expected_scope(authority: dict) -> dict:
    return {
        "included_prefixes": list(SOURCE_PREFIXES),
        "authority_gated_exceptions": sorted(allowed_authority_exceptions(authority)),
        "excluded_prefixes": list(EXCLUDED_PREFIXES),
        "exclusions": EXCLUSION_RECORDS,
        "file_extensions": sorted(SUFFIXES),
        "note": "Project-owned RT64 host/shader source plus explicitly authority-gated overlay files; all other contrib/tools trees remain excluded.",
    }


def assert_no_local_paths(value: object, where: str = "root") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            assert_no_local_paths(child, f"{where}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            assert_no_local_paths(child, f"{where}[{index}]")
    elif isinstance(value, str):
        require(LOCAL_PATH.search(value) is None, f"machine-local path leaked at {where}")
        require(not value.startswith(("/", "~/")), f"absolute path leaked at {where}")


def validate_snapshot(snapshot_value: object, known: set[str], label: str) -> None:
    require(isinstance(snapshot_value, dict), f"{label}: source snapshot must be an object")
    require(set(snapshot_value) == {"sha256", "lines", "candidate_hints", "dependencies"}, f"{label}: source snapshot fields changed")
    require(re.fullmatch(r"[0-9a-f]{64}", snapshot_value["sha256"]) is not None, f"{label}: invalid source digest")
    require(snapshot_value["sha256"] != "0" * 64, f"{label}: zero source digest")
    require(isinstance(snapshot_value["lines"], int) and snapshot_value["lines"] >= 0, f"{label}: invalid line count")
    hints = snapshot_value["candidate_hints"]
    require(isinstance(hints, list), f"{label}: candidate hints must be a list")
    require(hints == sorted(hints, key=lambda item: (item["name"], item["kind"])), f"{label}: candidate hints are not sorted")
    require(len({(item["kind"], item["name"]) for item in hints}) == len(hints), f"{label}: duplicate candidate hint")
    for hint in hints:
        require(set(hint) == {"kind", "name"}, f"{label}: candidate hint fields changed")
        require(hint["kind"] in {"type-declaration", "function-definition"}, f"{label}: invalid candidate hint kind")
        if hint["kind"] == "function-definition":
            require(hint["name"].rsplit("::", 1)[-1] not in NON_FUNCTION_HINTS, f"{label}: false-positive candidate hint")
    dependencies = snapshot_value["dependencies"]
    require(dependencies == sorted(set(dependencies)), f"{label}: dependencies are not sorted and unique")
    require(set(dependencies) <= known, f"{label}: dependency is absent from admitted denominator")


def validate_inventory(value: dict, authority: dict) -> None:
    expected_root = {
        "schema", "generated_by", "authority_manifest", "sources", "scope",
        "port_delta_counts", "source_set_sha256", "files",
    }
    require(set(value) == expected_root, "inventory root fields changed")
    require(value["schema"] == SCHEMA, "inventory schema changed")
    require(value["generated_by"] == "tools/rt64_port_inventory.py", "unexpected inventory generator")
    require(value["authority_manifest"] == "docs/rt64-port-authority.json", "authority manifest path changed")
    require(value["scope"] == expected_scope(authority), "inventory scope or exclusion boundary changed")
    sources = value["sources"]
    require(set(sources) == {"repository", "oracle", "port", "primary_port_input"}, "source fields changed")
    require(sources["repository"] == authority["repository"], "inventory repository drift")
    require(sources["oracle"] == source_identity(authority, "oracle"), "oracle identity drift")
    require(sources["port"] == source_identity(authority, "port"), "port-source identity drift")
    require(sources["primary_port_input"] == "port", "semantic port input drift")
    files = value["files"]
    require(isinstance(files, list) and files, "inventory contains no source files")
    paths = [item["path"] for item in files]
    require(paths == sorted(paths), "inventory file paths are not deterministically sorted")
    require(len(paths) == len(set(paths)), "duplicate inventory path")
    known = set(paths)
    known_by_source = {
        selection: {
            item["path"] for item in files
            if isinstance(item.get("sources"), dict) and item["sources"].get(selection) is not None
        }
        for selection in SOURCE_SELECTIONS
    }
    gates = {item["path"]: item for item in authority["overlays"]["source_gates"]}
    require(set(gates) <= known, f"authority gates missing from inventory: {sorted(set(gates) - known)}")
    destinations: set[str] = set()
    counts = {kind: 0 for kind in ("added", "removed", "modified", "unchanged")}
    for item in files:
        path = item["path"]
        base_keys = {"path", "sources", "port_delta", "milestone", "workstream", "port_state", "evidence_state", "task_card"}
        require(set(item) in (base_keys, base_keys | {"authority_gate"}), f"{path}: file entry fields changed")
        in_prefix = path.startswith(tuple(prefix + "/" for prefix in SOURCE_PREFIXES))
        require(in_prefix or path in allowed_authority_exceptions(authority), f"out-of-scope source path: {path}")
        require(not path.startswith(EXCLUDED_PREFIXES) or path in allowed_authority_exceptions(authority), f"excluded path in inventory: {path}")
        require(PurePosixPath(path).suffix in SUFFIXES, f"unexpected source suffix: {path}")
        source_values = item["sources"]
        require(set(source_values) == set(SOURCE_SELECTIONS), f"{path}: source snapshots changed")
        for selection in SOURCE_SELECTIONS:
            if source_values[selection] is not None:
                validate_snapshot(source_values[selection], known_by_source[selection], f"{path}:{selection}")
        expected_delta = delta_kind(source_values["oracle"], source_values["port"])
        require(item["port_delta"] == expected_delta, f"{path}: port delta classification drift")
        counts[expected_delta] += 1
        expected_route = route_for(path, gates)
        card = item["task_card"]
        require(isinstance(card, dict) and set(card) == TASK_KEYS, f"{path}: task-card fields changed")
        require((item["milestone"], item["workstream"], card["owner_lane"], card["recommended_profile"], item["port_state"]) == expected_route, f"{path}: audited route drift")
        require(item["milestone"] in MILESTONES, f"{path}: invalid milestone")
        require(item["evidence_state"] == "source-digests-verified", f"{path}: source evidence state drift")
        require(card["id"] == card_id(path, item["milestone"]), f"{path}: task-card id drift")
        require(set(card["authority"]) == {"port_source", "comparison_oracle", "plan"}, f"{path}: task authority fields changed")
        require(card["authority"]["port_source"] == authority_locator(authority, "port", path), f"{path}: port authority drift")
        require(card["authority"]["comparison_oracle"] == authority_locator(authority, "oracle", path), f"{path}: oracle authority drift")
        require(card["authority"]["plan"] == "docs/RENDER-WGPU-PORT-PLAN.md", f"{path}: plan authority drift")
        require(isinstance(card["outcome"], str) and path in card["outcome"], f"{path}: task outcome is not source-bound")
        require(card["non_goals"] and all(isinstance(text, str) for text in card["non_goals"]), f"{path}: task non-goals missing")
        require(card["baseline_command"].startswith("python3 tools/rt64_port_inventory.py --check"), f"{path}: baseline command drift")
        require(isinstance(card["exit_gate"], str) and item["milestone"] in card["exit_gate"], f"{path}: exit gate is not milestone-bound")
        require(card["evidence_state"] == "not-run", f"{path}: task evidence state drift")
        require(card["claim_status"] == "candidate-observation", f"{path}: task claim status drift")
        writable = card["writable_paths"]
        require(writable == [rust_destination(path, item["milestone"])], f"{path}: Rust writable destination drift")
        destination = writable[0]
        require(destination.startswith("crates/fn64-render"), f"{path}: task does not target Rust renderer source")
        require(destination not in destinations, f"duplicate writable destination: {destination}")
        destinations.add(destination)
        if path in gates:
            require("authority_gate" in item, f"{path}: authority gate metadata missing")
            require(item["authority_gate"] == {"mechanisms": gates[path]["mechanisms"], "oracle_sha256": gates[path]["sha256"]}, f"{path}: authority gate drift")
            require(source_values["oracle"] is not None and source_values["oracle"]["sha256"] == gates[path]["sha256"], f"{path}: authority oracle digest mismatch")
        else:
            require("authority_gate" not in item, f"{path}: spurious authority gate")
    require(value["port_delta_counts"] == counts, "port delta counts drift")
    require(re.fullmatch(r"[0-9a-f]{64}", value["source_set_sha256"]) is not None, "invalid source-set digest")
    require(value["source_set_sha256"] == source_set_digest(files), "source-set digest mismatch")
    require(value["source_set_sha256"] == EXPECTED_SOURCE_SET_SHA256, "pinned source-set digest drift")
    assert_no_local_paths(value)


def markdown(inventory: dict) -> str:
    files = inventory["files"]
    totals: dict[str, tuple[int, int]] = {}
    for item in files:
        primary = item["sources"]["port"] or item["sources"]["oracle"]
        count, lines = totals.get(item["milestone"], (0, 0))
        totals[item["milestone"]] = count + 1, lines + primary["lines"]
    sources = inventory["sources"]
    delta = inventory["port_delta_counts"]
    output = [
        "# RT64 port inventory", "",
        "<!-- Generated by tools/rt64_port_inventory.py from two admitted clean checkouts and docs/rt64-port-authority.json. -->", "",
        "This is the dual-pin mechanical work denominator for the RT64-to-Rust program. It records source identities, port deltas, include edges, non-exhaustive navigation hints, and dispatch-card contracts. It is not a behavior or parity claim.", "",
        "Regenerate or source-check it from explicit clean checkouts:", "",
        "```sh",
        "python3 tools/rt64_port_inventory.py --oracle-dir /absolute/path/to/clean/oracle --port-dir /absolute/path/to/clean/port-source",
        "python3 tools/rt64_port_inventory.py --check --oracle-dir /absolute/path/to/clean/oracle --port-dir /absolute/path/to/clean/port-source",
        "```", "",
        f"- Executable comparison oracle: [`{sources['oracle']['commit'][:7]}`]({sources['repository']}/commit/{sources['oracle']['commit']}) (`{sources['oracle']['authority_status']}`).",
        f"- Primary semantic port input: [`{sources['port']['commit'][:7]}`]({sources['repository']}/commit/{sources['port']['commit']}) (`{sources['port']['authority_status']}`).",
        f"- Denominator: {len(files)} project-owned or explicitly authority-gated host/shader files; `{sum((item['sources']['port'] or item['sources']['oracle'])['lines'] for item in files) / 1000:.3f}` KLOC at the primary port pin.",
        f"- Port delta: {delta['added']} added, {delta['removed']} removed, {delta['modified']} modified, {delta['unchanged']} unchanged source files.",
        f"- Source-set SHA-256: `{inventory['source_set_sha256']}`.",
        "- Excluded: all other `src/contrib/**` and `src/tools/**`. `src/tools/texture_hasher` and its GLIDEN64/Rice lineage, GPL `src/contrib/mupen64plus-core`, and m2c are never read as port authority.",
        "- Paths are repository-relative; the checked artifact rejects machine-local paths.", "",
        "`candidate_hints` in the JSON are deliberately non-exhaustive regex navigation aids, not a symbol denominator.", "",
        "## Milestone denominator", "", "| milestone | files | primary-port KLOC |", "|---|---:|---:|",
    ]
    for milestone in sorted(totals, key=lambda item: int(item[1:])):
        count, lines = totals[milestone]
        output.append(f"| `{milestone}` | {count} | `{lines / 1000:.3f}` |")
    output.extend(["", "## Source work cards", "", "Each row is one source-bound candidate card with a unique Rust destination. JSON carries its outcome, both authorities, exact destination, non-goals, baseline, exit gate, evidence state, and candidate-vs-claim status.", "", "| source | delta | lines | hints | deps | milestone / workstream | source evidence | task evidence / claim | owner | card |", "|---|---|---:|---:|---:|---|---|---|---|---|"])
    for item in files:
        primary = item["sources"]["port"] or item["sources"]["oracle"]
        card = item["task_card"]
        output.append(
            f"| `{item['path']}` | `{item['port_delta']}` | {primary['lines']} | {len(primary['candidate_hints'])} | {len(primary['dependencies'])} | "
            f"`{item['milestone']}` / `{item['workstream']}` | `{item['evidence_state']}` | `{card['evidence_state']}` / `{card['claim_status']}` | "
            f"`{card['owner_lane']}` ({card['recommended_profile']}) | `{card['id']}` |"
        )
    output.extend(["", "`authority-gated` is a source-overlay constraint, never completion evidence. Every task remains a candidate observation until its card exit gate and reliability bar pass.", ""])
    return "\n".join(output)


def canonical(value: dict) -> str:
    return json.dumps(value, indent=2, sort_keys=False) + "\n"


def expect_rejected(value: dict, authority: dict, needle: str) -> None:
    try:
        validate_inventory(value, authority)
    except InventoryError as error:
        require(needle in str(error), f"mutation failed for the wrong reason: {error}")
    else:
        raise InventoryError(f"mutation was accepted; expected {needle!r}")


def self_test() -> None:
    authority = load_authority()
    gates = {item["path"]: item for item in authority["overlays"]["source_gates"]}
    for unknown in ("src/common/new_unreviewed.cpp", "src/gui/new_unreviewed.h"):
        try:
            route_for(unknown, gates)
        except InventoryError as error:
            require("unrouted admitted" in str(error), "unknown M8 source was rejected for the wrong reason")
        else:
            raise InventoryError("unknown source silently fell through to M8")
    require(DEFAULT_JSON.is_file(), "committed inventory is required for mutation self-tests")
    base = json.loads(DEFAULT_JSON.read_text(encoding="utf-8"))
    validate_inventory(base, authority)
    mutated = copy.deepcopy(base)
    mutated["files"].reverse()
    expect_rejected(mutated, authority, "not deterministically sorted")
    mutated = copy.deepcopy(base)
    mutated["files"][0]["sources"]["oracle"]["sha256"] = "0" * 64
    expect_rejected(mutated, authority, "zero source digest")
    mutated = copy.deepcopy(base)
    mutated["files"][0]["sources"]["oracle"]["sha256"] = "f" * 64
    mutated["files"][0]["port_delta"] = "modified"
    mutated["port_delta_counts"]["unchanged"] -= 1
    mutated["port_delta_counts"]["modified"] += 1
    expect_rejected(mutated, authority, "source-set digest mismatch")
    mutated["source_set_sha256"] = source_set_digest(mutated["files"])
    expect_rejected(mutated, authority, "pinned source-set digest drift")
    mutated = copy.deepcopy(base)
    mutated["files"][0]["sources"]["oracle"]["dependencies"] = ["src/shared/not-real.h"]
    expect_rejected(mutated, authority, "dependency is absent")
    mutated = copy.deepcopy(base)
    mutated["files"][0]["task_card"]["writable_paths"] = ["/Users/example/private.rs"]
    expect_rejected(mutated, authority, "Rust writable destination drift")
    mutated = copy.deepcopy(base)
    mutated["files"][0]["task_card"]["outcome"] += " /Users/example/private.rs"
    expect_rejected(mutated, authority, "machine-local path leaked")
    mutated = copy.deepcopy(base)
    gate = authority["overlays"]["source_gates"][0]["path"]
    mutated["files"] = [item for item in mutated["files"] if item["path"] != gate]
    expect_rejected(mutated, authority, "authority gates missing")
    mutated = copy.deepcopy(base)
    mutated["files"][0]["task_card"].pop("exit_gate")
    expect_rejected(mutated, authority, "task-card fields changed")
    mutated = copy.deepcopy(base)
    routed = next(item for item in mutated["files"] if item["milestone"] == "M4" and "authority_gate" not in item)
    routed["milestone"] = "M8"
    expect_rejected(mutated, authority, "audited route drift")

    with tempfile.TemporaryDirectory() as temporary:
        tree = Path(temporary) / "rt64"
        tree.mkdir()
        subprocess.run(["git", "init", "-q", str(tree)], check=True)
        (tree / "LICENSE").write_text("fixture", encoding="utf-8")
        (tree / ".gitmodules").write_text("fixture", encoding="utf-8")
        (tree / "src/contrib/plume").mkdir(parents=True)
        subprocess.run(["git", "-C", str(tree), "add", "."], check=True)
        subprocess.run(["git", "-C", str(tree), "-c", "user.name=fn64", "-c", "user.email=fn64@example.invalid", "commit", "-qm", "fixture"], check=True)
        try:
            validate_tree(tree, authority, "oracle")
        except InventoryError as error:
            require("wrong authority pin" in str(error), "wrong-pin mutation was not rejected")
        else:
            raise InventoryError("wrong-pin mutation was accepted")
        dirty_authority = copy.deepcopy(authority)
        fixture_head = git(tree, "rev-parse", "HEAD")
        dirty_authority["oracle"]["commit"] = fixture_head
        dirty_authority["oracle"]["source_id"] = f"git:{fixture_head}"
        dirty_authority["oracle"]["license_sha256"] = digest(tree / "LICENSE")
        dirty_authority["oracle"]["gitmodules_sha256"] = digest(tree / ".gitmodules")
        (tree / "LICENSE").write_text("dirty", encoding="utf-8")
        try:
            validate_tree(tree, dirty_authority, "oracle")
        except InventoryError as error:
            require("checkout is dirty" in str(error), "dirty-tree mutation was not rejected")
        else:
            raise InventoryError("dirty-tree mutation was accepted")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--oracle-dir", type=Path, help="explicit clean executable-oracle checkout")
    parser.add_argument("--port-dir", type=Path, help="explicit clean accepted port-source checkout")
    parser.add_argument("--output", type=Path, default=DEFAULT_JSON, help="JSON inventory output path")
    parser.add_argument("--markdown-output", type=Path, default=DEFAULT_DOC, help="generated Markdown report path")
    parser.add_argument("--check", action="store_true", help="structurally check outputs; with both source dirs also rederive every byte")
    parser.add_argument("--self-test", action="store_true", help="run fail-closed mutation tests")
    arguments = parser.parse_args()
    try:
        if arguments.self_test:
            self_test()
            print("rt64-port-inventory: mutation self-tests clean")
            return 0
        authority = load_authority()
        supplied = (arguments.oracle_dir is not None, arguments.port_dir is not None)
        require(supplied[0] == supplied[1], "--oracle-dir and --port-dir must be supplied together")
        if supplied[0]:
            value = build_inventory(arguments.oracle_dir.resolve(), arguments.port_dir.resolve(), authority)
            validate_inventory(value, authority)
            expected_json = canonical(value)
            expected_doc = markdown(value)
            if arguments.check:
                require(arguments.output.is_file(), f"inventory is missing: {arguments.output}")
                require(arguments.output.read_text(encoding="utf-8") == expected_json, "inventory is stale; regenerate from both admitted checkouts")
                require(arguments.markdown_output.is_file(), f"generated report is missing: {arguments.markdown_output}")
                require(arguments.markdown_output.read_text(encoding="utf-8") == expected_doc, "generated report is stale; regenerate from both admitted checkouts")
            else:
                arguments.output.write_text(expected_json, encoding="utf-8")
                arguments.markdown_output.write_text(expected_doc, encoding="utf-8")
        else:
            require(arguments.check, "both source directories are required to generate; this tool never guesses machine-local checkouts")
            require(arguments.output.is_file(), f"inventory is missing: {arguments.output}")
            value = json.loads(arguments.output.read_text(encoding="utf-8"))
            validate_inventory(value, authority)
            require(arguments.markdown_output.is_file(), f"generated report is missing: {arguments.markdown_output}")
            require(arguments.markdown_output.read_text(encoding="utf-8") == markdown(value), "generated report is stale; regenerate from both admitted checkouts")
    except (InventoryError, OSError, json.JSONDecodeError, KeyError, TypeError) as error:
        print(f"rt64-port-inventory: {error}", file=sys.stderr)
        return 1
    print("rt64-port-inventory: clean")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
