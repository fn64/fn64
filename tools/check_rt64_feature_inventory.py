#!/usr/bin/env python3
"""Validate and render fn64's pinned RT64 public-feature inventory."""

from __future__ import annotations

import argparse
import copy
import json
import subprocess
import sys
from pathlib import Path


SCHEMA = "fn64.rt64-public-feature-inventory.v2"
BEHAVIOR_IDS = {
    "base-rendering-accuracy",
    "backend-d3d12",
    "backend-vulkan",
    "backend-metal",
    "ubershader-no-pipeline-stutter",
    "latency-skip-buffering",
    "latency-present-early",
    "high-resolution-renderer",
    "downsample-to-original-like",
    "widescreen-arbitrary-aspect",
    "ultrawide",
    "hfr-60-plus-interpolation",
    "extended-gbi",
    "texture-pack-dds",
    "texture-pack-rice-filenames",
    "texture-pack-async-streaming",
    "native-renderer-rdram-sync",
    "framebuffer-detection-region-copy",
    "framebuffer-upscaling",
    "framebuffer-reinterpretation",
    "debugger-frame-inspection",
    "deferred-frame-history",
    "platform-windows-10",
    "platform-windows-11",
    "platform-linux",
    "platform-macos",
}
STRATEGY_IDS = {
    "strategy-enhancement-oriented-architecture",
    "strategy-deferred-rdp",
    "strategy-deferred-rsp-compute",
    "strategy-texture-decoder-compute",
    "strategy-dual-renderers",
}
FUTURE_IDS = {
    "future-path-tracing",
    "future-model-replacements",
    "future-game-script-interpreter",
    "future-emulator-integration",
}
EXPECTED_IDS = BEHAVIOR_IDS | STRATEGY_IDS | FUTURE_IDS
ALLOWED_EVIDENCE = {
    "official_readme",
    "pinned_source",
    "fn64_feature_test",
    "fn64_base_pixel_test",
}
ALLOWED_CONTROLS = {
    "build_capability",
    "runtime_setting",
    "game_or_extended_gbi_cooperation",
}
REQUIRED_RUNTIME_SETTING_IDS = {
    "backend-d3d12",
    "backend-vulkan",
    "backend-metal",
    "latency-skip-buffering",
    "latency-present-early",
    "high-resolution-renderer",
    "downsample-to-original-like",
    "widescreen-arbitrary-aspect",
    "ultrawide",
    "hfr-60-plus-interpolation",
    "texture-pack-dds",
    "texture-pack-rice-filenames",
    "texture-pack-async-streaming",
    "debugger-frame-inspection",
}
EXPECTED_RUNTIME_FAMILIES = {
    "backend-d3d12": {"user_configuration"},
    "backend-vulkan": {"user_configuration"},
    "backend-metal": {"user_configuration"},
    "latency-skip-buffering": {"enhancement_configuration"},
    "latency-present-early": {"enhancement_configuration"},
    "high-resolution-renderer": {"user_configuration"},
    "downsample-to-original-like": {"user_configuration"},
    "widescreen-arbitrary-aspect": {"user_configuration"},
    "ultrawide": {"user_configuration"},
    "hfr-60-plus-interpolation": {"user_configuration"},
    "texture-pack-dds": {"texture_replacements"},
    "texture-pack-rice-filenames": {"texture_replacements"},
    "texture-pack-async-streaming": {"texture_replacements"},
    "debugger-frame-inspection": {"user_configuration"},
}
ALLOWED_RUNTIME_FAMILIES = {
    "user_configuration",
    "enhancement_configuration",
    "emulator_configuration",
    "texture_replacements",
}


class InventoryError(Exception):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise InventoryError(message)


def load_inventory(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise InventoryError(f"cannot read {path}: {error}") from error
    require(isinstance(value, dict), "inventory root must be an object")
    return value


def validate_shape(inventory: dict) -> dict[str, dict]:
    require(inventory.get("schema") == SCHEMA, f"schema must be {SCHEMA!r}")
    require(
        set(inventory) == {"schema", "upstream", "runtime_control_families", "items"},
        "unknown or missing root field",
    )

    upstream = inventory["upstream"]
    require(isinstance(upstream, dict), "upstream must be an object")
    require(
        set(upstream) == {"repository", "commit", "official_readme"},
        "unknown or missing upstream field",
    )
    require(upstream["repository"] == "https://github.com/rt64/rt64", "unexpected upstream repository")
    commit = upstream["commit"]
    require(
        isinstance(commit, str)
        and len(commit) == 40
        and all(character in "0123456789abcdef" for character in commit),
        "upstream commit must be a lowercase 40-character Git object ID",
    )
    require(
        upstream["official_readme"]
        == f"https://github.com/rt64/rt64/blob/{commit}/README.md",
        "official_readme must be pinned to upstream.commit",
    )

    runtime_families = inventory["runtime_control_families"]
    require(isinstance(runtime_families, dict), "runtime_control_families must be an object")
    require(
        set(runtime_families) == set(EXPECTED_RUNTIME_FAMILIES),
        "runtime_control_families keys must exactly match the runtime-setting behavior denominator",
    )
    for item_id, families in runtime_families.items():
        require(
            isinstance(families, list) and families,
            f"{item_id}: runtime control families must be a nonempty array",
        )
        require(
            len(families) == len(set(families)),
            f"{item_id}: runtime control families contain duplicates",
        )
        unknown_families = set(families) - ALLOWED_RUNTIME_FAMILIES
        require(
            not unknown_families,
            f"{item_id}: unknown runtime control families {sorted(unknown_families)}",
        )
        require(
            set(families) == EXPECTED_RUNTIME_FAMILIES[item_id],
            f"{item_id}: runtime control family classification drifted",
        )

    items = inventory["items"]
    require(isinstance(items, list), "items must be an array")
    by_id: dict[str, dict] = {}
    for index, item in enumerate(items):
        require(isinstance(item, dict), f"items[{index}] must be an object")
        require(
            set(item) == {"id", "title", "scope", "status", "controls", "closure", "evidence"},
            f"items[{index}] has unknown or missing field",
        )
        item_id = item["id"]
        require(isinstance(item_id, str) and item_id, f"items[{index}].id must be nonempty")
        require(item_id not in by_id, f"duplicate item id {item_id!r}")
        by_id[item_id] = item
        require(isinstance(item["title"], str) and item["title"].strip(), f"{item_id}: title is empty")
        require(isinstance(item["closure"], str) and item["closure"].strip(), f"{item_id}: closure is empty")
        controls = item["controls"]
        require(isinstance(controls, list) and controls, f"{item_id}: controls must be a nonempty array")
        require(len(controls) == len(set(controls)), f"{item_id}: controls contains duplicates")
        unknown_controls = set(controls) - ALLOWED_CONTROLS
        require(not unknown_controls, f"{item_id}: unknown controls {sorted(unknown_controls)}")
        if item_id in REQUIRED_RUNTIME_SETTING_IDS:
            require("runtime_setting" in controls, f"{item_id}: host feature must be classified as a runtime_setting")

        scope = item["scope"]
        status = item["status"]
        if item_id in BEHAVIOR_IDS:
            require(scope == "behavior", f"{item_id}: expected behavior scope")
            require(status in {"open", "closed"}, f"{item_id}: unknown behavior status {status!r}")
        elif item_id in STRATEGY_IDS:
            require(scope == "implementation_strategy", f"{item_id}: expected implementation_strategy scope")
            require(status == "documented_upstream", f"{item_id}: strategy status must be documented_upstream")
        elif item_id in FUTURE_IDS:
            require(scope == "upstream_future", f"{item_id}: expected upstream_future scope")
            require(status == "upstream_in_development", f"{item_id}: future status must be upstream_in_development")
        else:
            raise InventoryError(f"unknown item id {item_id!r}")

        evidence = item["evidence"]
        require(isinstance(evidence, list) and evidence, f"{item_id}: evidence must be nonempty")
        evidence_kinds: set[str] = set()
        feature_tests = 0
        for evidence_index, reference in enumerate(evidence):
            prefix = f"{item_id}.evidence[{evidence_index}]"
            require(isinstance(reference, dict), f"{prefix} must be an object")
            allowed_fields = {"kind", "path", "line", "needle", "claim_id"}
            require(set(reference) <= allowed_fields, f"{prefix} has an unknown field")
            require({"kind", "path", "line", "needle"} <= set(reference), f"{prefix} is incomplete")
            kind = reference["kind"]
            require(kind in ALLOWED_EVIDENCE, f"{prefix} has unknown evidence kind {kind!r}")
            evidence_kinds.add(kind)
            path = reference["path"]
            require(
                isinstance(path, str) and path and not Path(path).is_absolute() and ".." not in Path(path).parts,
                f"{prefix}.path must be a relative path without '..'",
            )
            require(isinstance(reference["line"], int) and reference["line"] > 0, f"{prefix}.line must be positive")
            require(isinstance(reference["needle"], str) and reference["needle"], f"{prefix}.needle is empty")
            if kind == "official_readme":
                require(path == "README.md", f"{prefix}: official README evidence must use README.md")
            if kind == "fn64_feature_test":
                feature_tests += 1
                require(
                    reference.get("claim_id") == item_id,
                    f"{prefix}: feature test claim_id must equal its inventory item id",
                )
            else:
                require("claim_id" not in reference, f"{prefix}: claim_id is only valid for fn64_feature_test")

        require("official_readme" in evidence_kinds, f"{item_id}: official README evidence is required")
        if scope != "upstream_future":
            require("pinned_source" in evidence_kinds, f"{item_id}: pinned source evidence is required")
        if scope == "behavior" and status == "closed":
            require(
                feature_tests > 0,
                f"{item_id}: closed behavior needs a feature-specific fn64_feature_test; base-pixel evidence never closes an enhancement",
            )

    actual_ids = set(by_id)
    missing = sorted(EXPECTED_IDS - actual_ids)
    unknown = sorted(actual_ids - EXPECTED_IDS)
    require(not missing, f"missing advertised inventory ids: {', '.join(missing)}")
    require(not unknown, f"unknown inventory ids: {', '.join(unknown)}")
    actual_runtime_ids = {
        item_id
        for item_id, item in by_id.items()
        if item_id in BEHAVIOR_IDS and "runtime_setting" in item["controls"]
    }
    require(
        actual_runtime_ids == set(runtime_families),
        "runtime_control_families must cover every and only runtime-setting item",
    )
    return by_id


def validate_pinned_tree(inventory: dict, rt64_dir: Path) -> None:
    require(rt64_dir.is_dir(), f"RT64 source directory does not exist: {rt64_dir}")
    try:
        head = subprocess.run(
            ["git", "-C", str(rt64_dir), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise InventoryError(f"cannot resolve RT64 source identity: {error}") from error
    require(head == inventory["upstream"]["commit"], f"RT64 HEAD {head} does not match pinned commit")
    try:
        dirty = subprocess.run(
            ["git", "-C", str(rt64_dir), "status", "--porcelain", "--", "."],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        license_text = (rt64_dir / "LICENSE").read_text(encoding="utf-8")
    except (OSError, subprocess.CalledProcessError) as error:
        raise InventoryError(f"cannot audit pinned RT64 source tree: {error}") from error
    require(not dirty, "RT64 source tree is dirty; inventory provenance requires the exact pinned tree")
    require("MIT License" in license_text, "RT64 pinned source does not carry the expected MIT license")

    for item in inventory["items"]:
        for reference in item["evidence"]:
            if reference["kind"] not in {"official_readme", "pinned_source"}:
                continue
            source_path = rt64_dir / reference["path"]
            try:
                lines = source_path.read_text(encoding="utf-8").splitlines()
            except OSError as error:
                raise InventoryError(f"{item['id']}: cannot read {source_path}: {error}") from error
            line = reference["line"]
            require(line <= len(lines), f"{item['id']}: {reference['path']}:{line} is past EOF")
            require(
                reference["needle"] in lines[line - 1],
                f"{item['id']}: evidence needle absent at {reference['path']}:{line}",
            )


def validate_local_evidence(inventory: dict, root: Path) -> None:
    for item in inventory["items"]:
        for reference in item["evidence"]:
            if reference["kind"] not in {"fn64_feature_test", "fn64_base_pixel_test"}:
                continue
            source_path = root / reference["path"]
            try:
                lines = source_path.read_text(encoding="utf-8").splitlines()
            except OSError as error:
                raise InventoryError(
                    f"{item['id']}: cannot read local evidence {source_path}: {error}"
                ) from error
            line = reference["line"]
            require(
                line <= len(lines),
                f"{item['id']}: local evidence {reference['path']}:{line} is past EOF",
            )
            require(
                reference["needle"] in lines[line - 1],
                f"{item['id']}: local evidence needle absent at {reference['path']}:{line}",
            )


def validate_rejection_guards(inventory: dict) -> None:
    def expect_rejected(label: str, mutation) -> None:
        candidate = copy.deepcopy(inventory)
        mutation(candidate)
        try:
            validate_shape(candidate)
        except InventoryError:
            return
        raise InventoryError(f"validator rejection guard failed: {label}")

    expect_rejected(
        "missing advertised ID",
        lambda candidate: candidate["items"].pop(),
    )
    expect_rejected(
        "unknown status",
        lambda candidate: candidate["items"][0].__setitem__("status", "looks-good"),
    )
    expect_rejected(
        "unknown evidence kind",
        lambda candidate: candidate["items"][0]["evidence"][0].__setitem__("kind", "handwave"),
    )
    expect_rejected(
        "runtime setting reclassified as generated-game policy",
        lambda candidate: next(
            item for item in candidate["items"] if item["id"] == "backend-d3d12"
        ).__setitem__("controls", ["game_or_extended_gbi_cooperation"]),
    )
    expect_rejected(
        "runtime control family misclassified",
        lambda candidate: candidate["runtime_control_families"].__setitem__(
            "latency-present-early", ["user_configuration"]
        ),
    )

    def close_with_base_pixels_only(candidate: dict) -> None:
        item = next(entry for entry in candidate["items"] if entry["id"] == "backend-d3d12")
        item["status"] = "closed"
        item["evidence"] = [
            reference for reference in item["evidence"] if reference["kind"] != "fn64_feature_test"
        ]
        item["evidence"].append(
            {
                "kind": "fn64_base_pixel_test",
                "path": "crates/fn64-render-rt64/tests/base_pixel.rs",
                "line": 1,
                "needle": "base pixels",
            }
        )

    expect_rejected("base pixels used to close an enhancement", close_with_base_pixels_only)


def evidence_link(inventory: dict, reference: dict) -> str:
    path = reference["path"]
    line = reference["line"]
    if reference["kind"] in {"fn64_feature_test", "fn64_base_pixel_test"}:
        url = f"../{path}#L{line}"
    else:
        commit = inventory["upstream"]["commit"]
        url = f"https://github.com/rt64/rt64/blob/{commit}/{path}#L{line}"
    return f"[{path}:{line}]({url})"


def render_doc(inventory: dict, by_id: dict[str, dict]) -> str:
    behavior = [by_id[item_id] for item_id in sorted(BEHAVIOR_IDS)]
    strategies = [by_id[item_id] for item_id in sorted(STRATEGY_IDS)]
    futures = [by_id[item_id] for item_id in sorted(FUTURE_IDS)]
    closed = sum(item["status"] == "closed" for item in behavior)
    open_count = len(behavior) - closed
    commit = inventory["upstream"]["commit"]

    def control_text(item: dict) -> str:
        labels = {
            "build_capability": "build capability",
            "runtime_setting": "runtime setting",
            "game_or_extended_gbi_cooperation": "game/Extended-GBI cooperation",
        }
        controls = []
        family_labels = {
            "user_configuration": "UserConfiguration",
            "enhancement_configuration": "EnhancementConfiguration",
            "emulator_configuration": "EmulatorConfiguration",
            "texture_replacements": "texture replacements",
        }
        for value in item["controls"]:
            label = labels[value]
            if value == "runtime_setting" and item["id"] in inventory["runtime_control_families"]:
                families = inventory["runtime_control_families"][item["id"]]
                label += " (" + ", ".join(family_labels[family] for family in families) + ")"
            controls.append(label)
        return ", ".join(controls)

    lines = [
        "# RT64 public feature inventory",
        "",
        "Generated by `tools/check_rt64_feature_inventory.py` from",
        "`docs/rt64-public-feature-inventory.json`; do not edit this file by hand.",
        "",
        f"Upstream is the official MIT RT64 repository pinned at `{commit}`.",
        "`closed` means fn64 has feature-specific behavioral evidence for that exact",
        "claim. Base pixel accuracy, adapter construction, or a post-VI capture cannot",
        "close an enhancement claim. Implementation strategies and upstream future work",
        "are tracked separately and are excluded from the behavior denominator.",
        "Runtime settings belong to the fn64 host/configuration surface, not generated game",
        "code. Only rows explicitly classified as game/Extended-GBI cooperation require",
        "game-side participation. Runtime-setting rows name their exact pinned RT64 control",
        "family; see `RT64-RUNTIME-CONTROLS.md` for the complete boundary.",
        "",
        "## Denominator",
        "",
        "| Scope | Closed | Open | Tracked |",
        "|---|---:|---:|---:|",
        f"| Available behavior | {closed} | {open_count} | {len(behavior)} |",
        f"| Implementation strategy | n/a | n/a | {len(strategies)} |",
        f"| Upstream in development | n/a | n/a | {len(futures)} |",
        "",
        f"Exact open denominator: **{open_count}/{len(behavior)} available behaviors**.",
        "",
        "## Available behavior claims",
        "",
        "| ID | Claim | Control | Status | Upstream evidence | Closure frontier |",
        "|---|---|---|---|---|---|",
    ]
    for item in behavior:
        links = ", ".join(evidence_link(inventory, ref) for ref in item["evidence"])
        lines.append(f"| `{item['id']}` | {item['title']} | {control_text(item)} | `{item['status']}` | {links} | {item['closure']} |")

    lines.extend(
        [
            "",
            "## Implementation strategies",
            "",
            "These describe how upstream RT64 is built. fn64 need not reproduce the same",
            "architecture to reach behavioral parity.",
            "",
            "| ID | Upstream strategy | Control | Evidence |",
            "|---|---|---|---|",
        ]
    )
    for item in strategies:
        links = ", ".join(evidence_link(inventory, ref) for ref in item["evidence"] if ref["kind"] in {"official_readme", "pinned_source"})
        lines.append(f"| `{item['id']}` | {item['title']} | {control_text(item)} | {links} |")

    lines.extend(
        [
            "",
            "## Upstream in development",
            "",
            "These are not advertised as available in the pinned repository and do not",
            "enter fn64's available-feature denominator.",
            "",
            "| ID | Upstream item | Anticipated control | Evidence |",
            "|---|---|---|---|",
        ]
    )
    for item in futures:
        links = ", ".join(evidence_link(inventory, ref) for ref in item["evidence"] if ref["kind"] == "official_readme")
        lines.append(f"| `{item['id']}` | {item['title']} | {control_text(item)} | {links} |")

    lines.extend(
        [
            "",
            "## Validation",
            "",
            "```sh",
            "python3 tools/check_rt64_feature_inventory.py --rt64-dir ../no-mercy-recompiled/third_party/rt64",
            "```",
            "",
            "The check rejects missing inventory IDs, unknown scopes/statuses/evidence",
            "kinds/control classes, loss of required runtime-setting classifications, a",
            "dirty or non-MIT source tree, source drift from the pinned commit, stale line",
            "anchors, generated-doc drift, and any closed enhancement lacking a",
            "feature-specific fn64 test.",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--inventory",
        type=Path,
        default=Path("docs/rt64-public-feature-inventory.json"),
    )
    parser.add_argument(
        "--doc",
        type=Path,
        default=Path("docs/RT64-PUBLIC-FEATURE-INVENTORY.md"),
    )
    parser.add_argument("--rt64-dir", type=Path)
    parser.add_argument("--print-doc", action="store_true")
    parser.add_argument("--write-doc", action="store_true")
    args = parser.parse_args()

    try:
        inventory = load_inventory(args.inventory)
        by_id = validate_shape(inventory)
        validate_rejection_guards(inventory)
        validate_local_evidence(inventory, Path(__file__).resolve().parent.parent)
        if args.rt64_dir is not None:
            validate_pinned_tree(inventory, args.rt64_dir)
        rendered = render_doc(inventory, by_id)
        require(not (args.print_doc and args.write_doc), "--print-doc and --write-doc are mutually exclusive")
        if args.print_doc:
            sys.stdout.write(rendered)
            return 0
        if args.write_doc:
            try:
                args.doc.write_text(rendered, encoding="utf-8")
            except OSError as error:
                raise InventoryError(f"cannot write generated doc {args.doc}: {error}") from error
            print(f"rt64-feature-inventory: wrote {args.doc}")
            return 0
        try:
            checked_doc = args.doc.read_text(encoding="utf-8")
        except OSError as error:
            raise InventoryError(f"cannot read generated doc {args.doc}: {error}") from error
        require(checked_doc == rendered, f"generated doc is stale: {args.doc}; inspect with --print-doc")
    except InventoryError as error:
        print(f"rt64-feature-inventory: {error}", file=sys.stderr)
        return 1

    closed = sum(by_id[item_id]["status"] == "closed" for item_id in BEHAVIOR_IDS)
    print(
        "rt64-feature-inventory: clean "
        f"({closed} closed, {len(BEHAVIOR_IDS) - closed}/{len(BEHAVIOR_IDS)} behavior open; "
        f"{len(STRATEGY_IDS)} strategies; {len(FUTURE_IDS)} upstream-development items)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
