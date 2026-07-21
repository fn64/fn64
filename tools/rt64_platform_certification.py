#!/usr/bin/env python3
"""Validate, render, execute, and verify cross-platform RT64 certification."""

from __future__ import annotations

import argparse
import copy
import datetime as dt
import hashlib
import json
import os
import platform
import subprocess
import sys
from pathlib import Path


MANIFEST_SCHEMA = "fn64.rt64-platform-certification.v1"
RESULT_SCHEMA = "fn64.rt64-platform-certification-result.v1"
LIVE_CASE_TIMEOUT_SECONDS = 60
LEGACY_MACOS_CASES = {
    "backend-lifecycle",
    "resolution-downsample",
    "framebuffer-rdram-region",
    "framebuffer-enhancement",
    "texture-replacements",
    "latency-skip-buffering",
    "latency-present-early",
    "deferred-debugger",
    "ubershader-critical-path",
    "hfr-hle-cooperation",
    "extended-gbi-cooperation",
}
EXPECTED_CASES = LEGACY_MACOS_CASES | {
    "user-controls-rebuild",
    "enhancement-emulator-controls",
}
EXPECTED_BLOCKERS = {
    "recognized-hle-and-extended-gbi",
    "aspect-and-generated-frames",
    "remaining-user-controls",
    "remaining-enhancement-controls",
    "inspector-gui",
    "full-adapter-rom-coverage",
    "declared-host-range",
}
EXPECTED_TARGETS = {
    "macos-metal": ("platform-macos", "macos", "metal"),
    "linux-vulkan": ("platform-linux", "linux", "vulkan"),
    "windows10-d3d12": ("platform-windows-10", "windows10", "d3d12"),
    "windows10-vulkan": ("platform-windows-10", "windows10", "vulkan"),
    "windows11-d3d12": ("platform-windows-11", "windows11", "d3d12"),
    "windows11-vulkan": ("platform-windows-11", "windows11", "vulkan"),
}
ROOT_FIELDS = {
    "schema", "source", "legacy_macos_manifest", "denominator", "cases",
    "blockers", "targets",
}
SOURCE_FIELDS = {"rt64_commit", "source_id", "provenance"}
CASE_FIELDS = {"id", "category", "example", "features", "repeat_bar", "claims"}
RECORDED_CASE_FIELDS = {"status", "clean_runs", "verified_on", "host", "note"}
BLOCKER_FIELDS = {"id", "claims", "description"}
TARGET_FIELDS = {
    "id", "platform_claim", "os_family", "graphics_api", "capture_api",
    "status", "case_statuses", "open_blockers", "frontier",
}
RESULT_FIELDS = {
    "schema", "target", "case", "source_id", "host", "requested_runs",
    "repeat_bar", "clean_runs", "iterations", "status", "reason",
    "recorded_at_utc", "result_sha256",
}
HOST_FIELDS = {
    "os_family", "os_product", "os_version", "os_build", "kernel",
    "architecture", "gpu", "graphics_api",
}
LEGACY_HOST_FIELDS = {"product", "version", "build", "kernel", "architecture", "gpu"}
RESULT_STATUSES = {"repeat-bar-passed", "diagnostic-only", "failed", "blocked", "skipped"}


class CertificationError(Exception):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise CertificationError(message)


def load_json(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise CertificationError(f"cannot read {path}: {error}") from error
    require(isinstance(value, dict), f"{path}: root must be an object")
    return value


def nonempty(value: object, where: str) -> str:
    require(isinstance(value, str) and bool(value.strip()), f"{where} must be nonempty")
    return value


def unique_strings(value: object, where: str) -> list[str]:
    require(isinstance(value, list), f"{where} must be an array")
    require(all(isinstance(item, str) and item for item in value), f"{where} entries must be nonempty strings")
    require(len(value) == len(set(value)), f"{where} contains duplicates")
    return value


def inventory(root: Path) -> dict[str, dict]:
    items = load_json(root / "docs/rt64-public-feature-inventory.json").get("items")
    require(isinstance(items, list), "public inventory items must be an array")
    by_id = {item.get("id"): item for item in items if isinstance(item, dict)}
    require(len(by_id) == len(items), "public inventory contains duplicate or invalid IDs")
    return by_id


def validate_legacy_macos(manifest: dict, cases: dict[str, dict], root: Path) -> dict:
    legacy_path = root / nonempty(manifest["legacy_macos_manifest"], "legacy_macos_manifest")
    legacy = load_json(legacy_path)
    require(legacy.get("schema") == "fn64.rt64-macos-certification.v1", "legacy macOS schema drifted")
    require(legacy.get("source") == {**manifest["source"], "post_vi_api": "metal-bgra8-unorm"}, "legacy macOS source identity drifted")
    host = legacy.get("recorded_host")
    require(isinstance(host, dict) and set(host) == LEGACY_HOST_FIELDS, "legacy macOS host provenance fields drifted")
    for field in LEGACY_HOST_FIELDS:
        nonempty(host[field], f"legacy recorded_host.{field}")
    require(host["product"] == "macOS", "legacy recorded host is not macOS")
    legacy_cases = legacy.get("cases")
    require(isinstance(legacy_cases, list), "legacy macOS cases must be an array")
    by_id = {case.get("id"): case for case in legacy_cases if isinstance(case, dict)}
    require(set(by_id) == LEGACY_MACOS_CASES, "legacy macOS eleven-case denominator drifted")
    for case_id in LEGACY_MACOS_CASES:
        case = cases[case_id]
        old = by_id[case_id]
        for field in ("category", "example", "features", "repeat_bar", "claims"):
            require(old.get(field) == case[field], f"legacy macOS {case_id}.{field} drifted")
        recorded = old.get("recorded")
        require(isinstance(recorded, dict) and recorded.get("status") == "pass", f"legacy macOS {case_id} lost pass evidence")
        require(recorded.get("clean_runs", 0) >= case["repeat_bar"], f"legacy macOS {case_id} fell below repeat bar")
    return legacy


def validate_manifest(manifest: dict, root: Path) -> tuple[dict[str, dict], dict[str, dict], dict[str, dict], dict]:
    require(set(manifest) == ROOT_FIELDS, "unknown or missing manifest root field")
    require(manifest["schema"] == MANIFEST_SCHEMA, f"schema must be {MANIFEST_SCHEMA!r}")
    nonempty(manifest["denominator"], "denominator")
    source = manifest["source"]
    require(isinstance(source, dict) and set(source) == SOURCE_FIELDS, "invalid source fields")
    commit = nonempty(source["rt64_commit"], "source.rt64_commit")
    require(len(commit) == 40 and all(c in "0123456789abcdef" for c in commit), "source.rt64_commit must be a lowercase full commit")
    require(source["source_id"] == f"git:{commit}", "source.source_id must name the pinned commit")
    require(source["provenance"] == "GitClean", "source.provenance must be GitClean")

    public = inventory(root)
    cargo_toml = (root / "crates/fn64-render-rt64/Cargo.toml").read_text(encoding="utf-8")
    cases: dict[str, dict] = {}
    require(isinstance(manifest["cases"], list), "cases must be an array")
    for index, case in enumerate(manifest["cases"]):
        where = f"cases[{index}]"
        require(
            isinstance(case, dict)
            and frozenset(case)
            in {frozenset(CASE_FIELDS), frozenset(CASE_FIELDS | {"recorded"})},
            f"{where}: invalid fields",
        )
        case_id = nonempty(case["id"], f"{where}.id")
        require(case_id not in cases, f"duplicate case {case_id!r}")
        require(case["repeat_bar"] in {10, 20}, f"{case_id}: repeat_bar must be 10 or 20")
        example = nonempty(case["example"], f"{case_id}.example")
        example_path = root / f"crates/fn64-render-rt64/examples/{example}.rs"
        require(example_path.is_file(), f"{case_id}: missing example")
        require(f'name = "{example}"' in cargo_toml, f"{case_id}: example absent from Cargo.toml")
        require(source["source_id"] in example_path.read_text(encoding="utf-8"), f"{case_id}: example does not enforce pinned source identity")
        features = unique_strings(case["features"], f"{case_id}.features")
        require(features, f"{case_id}: features must not be empty")
        for claim in unique_strings(case["claims"], f"{case_id}.claims"):
            require(claim in public, f"{case_id}: unknown claim {claim!r}")
            require(public[claim].get("status") == "closed", f"{case_id}: preserved macOS claim {claim!r} is no longer closed")
        recorded = case.get("recorded")
        if recorded is not None:
            require(
                isinstance(recorded, dict) and set(recorded) == RECORDED_CASE_FIELDS,
                f"{case_id}.recorded: invalid fields",
            )
            require(recorded["status"] == "pass", f"{case_id}: recorded status must be pass")
            require(
                isinstance(recorded["clean_runs"], int)
                and recorded["clean_runs"] >= case["repeat_bar"],
                f"{case_id}: recorded evidence fell below repeat bar",
            )
            nonempty(recorded["verified_on"], f"{case_id}.recorded.verified_on")
            nonempty(recorded["note"], f"{case_id}.recorded.note")
            host = recorded["host"]
            require(
                isinstance(host, dict) and set(host) == LEGACY_HOST_FIELDS,
                f"{case_id}.recorded.host: invalid fields",
            )
            for field in LEGACY_HOST_FIELDS:
                nonempty(host[field], f"{case_id}.recorded.host.{field}")
        cases[case_id] = case
    require(set(cases) == EXPECTED_CASES, f"case denominator drifted: missing={sorted(EXPECTED_CASES - set(cases))}, extra={sorted(set(cases) - EXPECTED_CASES)}")

    blockers: dict[str, dict] = {}
    require(isinstance(manifest["blockers"], list), "blockers must be an array")
    for index, blocker in enumerate(manifest["blockers"]):
        where = f"blockers[{index}]"
        require(isinstance(blocker, dict) and set(blocker) == BLOCKER_FIELDS, f"{where}: invalid fields")
        blocker_id = nonempty(blocker["id"], f"{where}.id")
        require(blocker_id not in blockers, f"duplicate blocker {blocker_id!r}")
        nonempty(blocker["description"], f"{blocker_id}.description")
        for claim in unique_strings(blocker["claims"], f"{blocker_id}.claims"):
            require(claim in public, f"{blocker_id}: unknown claim {claim!r}")
            require(public[claim].get("status") == "open", f"{blocker_id}: blocker claim {claim!r} is not open")
        blockers[blocker_id] = blocker
    require(set(blockers) == EXPECTED_BLOCKERS, f"blocker denominator drifted: missing={sorted(EXPECTED_BLOCKERS - set(blockers))}, extra={sorted(set(blockers) - EXPECTED_BLOCKERS)}")

    targets: dict[str, dict] = {}
    require(isinstance(manifest["targets"], list), "targets must be an array")
    for index, target in enumerate(manifest["targets"]):
        where = f"targets[{index}]"
        require(isinstance(target, dict) and set(target) == TARGET_FIELDS, f"{where}: invalid fields")
        target_id = nonempty(target["id"], f"{where}.id")
        require(target_id not in targets, f"duplicate target {target_id!r}")
        expected = EXPECTED_TARGETS.get(target_id)
        require(expected == (target["platform_claim"], target["os_family"], target["graphics_api"]), f"{target_id}: target identity drifted")
        require(target["status"] == "open", f"{target_id}: platform/API target must remain open")
        require(public.get(target["platform_claim"], {}).get("status") == "open", f"{target_id}: inventory platform claim must remain open")
        require(isinstance(target["case_statuses"], dict) and set(target["case_statuses"]) == EXPECTED_CASES, f"{target_id}: per-platform case denominator drifted")
        require(set(target["case_statuses"].values()) <= {"pass", "blocked", "skipped"}, f"{target_id}: invalid case status")
        require(set(unique_strings(target["open_blockers"], f"{target_id}.open_blockers")) == EXPECTED_BLOCKERS, f"{target_id}: per-platform blocker denominator drifted")
        nonempty(target["capture_api"], f"{target_id}.capture_api")
        nonempty(target["frontier"], f"{target_id}.frontier")
        if target_id == "macos-metal":
            require(set(target["case_statuses"].values()) == {"pass"}, "macOS Metal must retain actual evidence for every pass")
            require(target["capture_api"] == "metal-bgra8-unorm", "macOS capture API drifted")
            for case_id, case in cases.items():
                require(
                    case_id in LEGACY_MACOS_CASES or case.get("recorded") is not None,
                    f"macOS Metal pass {case_id!r} has no retained actual-hardware evidence",
                )
        else:
            require("pass" not in target["case_statuses"].values(), f"{target_id}: unavailable host evidence cannot pass")
            expected_capture_api = (
                "vulkan-bgra8-rgba8-unorm"
                if target["graphics_api"] == "vulkan"
                else "d3d12-bgra8-rgba8-unorm"
            )
            require(
                target["capture_api"] == expected_capture_api,
                f"{target_id}: portable capture API identity drifted",
            )
            for case_id in EXPECTED_CASES - LEGACY_MACOS_CASES:
                require(
                    target["case_statuses"][case_id] == "blocked",
                    f"{target_id}: {case_id} must be explicitly blocked without hardware evidence",
                )
        targets[target_id] = target
    require(set(targets) == set(EXPECTED_TARGETS), f"target denominator drifted: missing={sorted(set(EXPECTED_TARGETS) - set(targets))}, extra={sorted(set(targets) - set(EXPECTED_TARGETS))}")
    legacy = validate_legacy_macos(manifest, cases, root)
    return cases, blockers, targets, legacy


def legacy_recorded(legacy: dict) -> dict[str, dict]:
    return {case["id"]: case["recorded"] for case in legacy["cases"]}


def render_doc(manifest: dict, cases: dict[str, dict], blockers: dict[str, dict], targets: dict[str, dict], legacy: dict) -> str:
    source = manifest["source"]
    old = legacy_recorded(legacy)
    lines = [
        "# RT64 cross-platform certification",
        "",
        "Generated from `docs/rt64-platform-certification.json` by",
        "`tools/rt64_platform_certification.py`; edit the JSON, not this file.",
        "",
        "## Status",
        "",
        "**Every platform row remains open.** A build-capability advertisement is not",
        "actual-hardware certification. Blocked and skipped states never count as passes.",
        "The existing eleven-case macOS/Metal evidence is preserved exactly from",
        "`docs/rt64-macos-certification.json`; post-legacy cases carry their own retained",
        "macOS evidence below. No Linux, Windows, Vulkan, or D3D12 hardware result is",
        "inferred from either source.",
        "",
        f"Pinned RT64 source: `{source['source_id']}` (`{source['provenance']}`).",
        "",
        f"Preserved macOS host: {legacy['recorded_host']['product']} {legacy['recorded_host']['version']} build {legacy['recorded_host']['build']}; {legacy['recorded_host']['kernel']} {legacy['recorded_host']['architecture']}; {legacy['recorded_host']['gpu']}; Metal.",
        "",
        f"Denominator: {manifest['denominator']}",
        "",
        "## Platform/API targets",
        "",
        "| Target | Platform claim | API | Capture | Cases | Blockers | Exact frontier |",
        "|---|---|---|---|---:|---:|---|",
    ]
    for target in targets.values():
        counts = {state: list(target["case_statuses"].values()).count(state) for state in ("pass", "blocked", "skipped")}
        case_summary = f"{counts['pass']} pass / {counts['blocked']} blocked / {counts['skipped']} skipped"
        lines.append(f"| `{target['id']}` | `{target['platform_claim']}` (`open`) | `{target['graphics_api']}` | `{target['capture_api']}` | {case_summary} | {len(target['open_blockers'])} open | {target['frontier']} |")
    lines.extend([
        "",
        "## Non-shrinking case matrix",
        "",
        "| Case | Repeat bar | macOS/Metal | Linux/Vulkan | Win10/D3D12 | Win10/Vulkan | Win11/D3D12 | Win11/Vulkan |",
        "|---|---:|---|---|---|---|---|---|",
    ])
    target_order = list(EXPECTED_TARGETS)
    for case in cases.values():
        cells = []
        for target_id in target_order:
            status = targets[target_id]["case_statuses"][case["id"]]
            if target_id == "macos-metal" and status == "pass":
                record = old.get(case["id"], case.get("recorded"))
                cells.append(f"pass: {record['clean_runs']} clean ({record['verified_on']})")
            else:
                cells.append(status)
        lines.append(f"| `{case['id']}` | {case['repeat_bar']} | " + " | ".join(cells) + " |")
    lines.extend([
        "",
        "The `user-controls-rebuild` race fix passed twenty consecutive watchdog-bounded",
        "full process exits with exact policy and pixel digests. The bounded failures at",
        "`/tmp/fn64-rt64-control-89452.sample.txt` and",
        "`/tmp/fn64-rt64-control-86646.sample.txt` showed `Application::end` joining a",
        "raster-shader worker after its delayed startup overwrote the destructor's stop",
        "predicate and slept after the only notification. Exact-source overlay",
        "`fn64:raster-shader-start-stop:v1` publishes the predicate before launch and",
        "leaves teardown as its only post-launch writer. The 20 retained run logs are",
        "`/tmp/fn64-rt64-user-controls-overlay-run-{1..20}.log`.",
        "",
        "The `enhancement-emulator-controls` pass uses isolated fresh contexts and",
        "hard per-process watchdogs. Its retained note preserves discarded exploratory",
        "capture, cross-profile-contamination, and non-mechanism copy observations, then",
        "binds the final two-workload fixture to exclusive GPU-tile-copy versus ordinary",
        "RDRAM/TMEM-upload paths through a read-only completed-workload seam.",
        "",
        "## Non-shrinking blocker denominator",
        "",
        "Every target carries all seven blockers below. Removing a case, blocker, or target",
        "fails static validation; closing one requires a retained, integrity-checked result",
        "from matching hardware and does not close any other row.",
        "",
        "| Blocker | Related claims | Frontier |",
        "|---|---|---|",
    ])
    for blocker in blockers.values():
        claims = ", ".join(f"`{claim}`" for claim in blocker["claims"]) or "—"
        lines.append(f"| `{blocker['id']}` | {claims} | {blocker['description']} |")
    lines.extend([
        "",
        "## CI and actual-hardware commands",
        "",
        "GPU-free validation and planning:",
        "",
        "```sh",
        "python3 tools/rt64_platform_certification.py --check",
        "python3 tools/rt64_platform_certification.py --selftest",
        "python3 tools/rt64_platform_certification.py --list",
        "python3 tools/rt64_platform_certification.py --plan linux-vulkan",
        "python3 tools/rt64_platform_certification.py --verify-result path/to/result.json",
        "```",
        "",
        "A matching actual-hardware runner retains one integrity-bound result:",
        "",
        "```sh",
        "python3 tools/rt64_platform_certification.py \\",
        "  --run macos-metal:backend-lifecycle --gpu 'Apple M5 Pro' \\",
        "  --rt64-dir /absolute/path/to/clean/pinned/rt64 \\",
        "  --result artifacts/macos-metal-backend-lifecycle.json",
        "```",
        "",
        "A shorter `--runs 1` result is `diagnostic-only`. A host/target mismatch is",
        "retained as `skipped`; a matching target whose case is not runnable is retained",
        "as `blocked`. Neither can satisfy a repeat bar. Results bind source identity,",
        "OS product/version/build/kernel, architecture, GPU, graphics API, every process",
        "exit code, run count, status, reason, and a canonical SHA-256. Live execution",
        "requires an explicit RT64 directory; tooling never guesses an out-of-tree path.",
        f"Every live case invocation fails if it exceeds the shared {LIVE_CASE_TIMEOUT_SECONDS}-second",
        "per-process watchdog.",
        "Skipped/blocked execution exits 2 so CI cannot mistake it for a passing run.",
        "",
    ])
    return "\n".join(lines)


def canonical_result(result: dict) -> bytes:
    semantic = {key: value for key, value in result.items() if key != "result_sha256"}
    return json.dumps(semantic, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def sign_result(result: dict) -> dict:
    result["result_sha256"] = hashlib.sha256(canonical_result(result)).hexdigest()
    return result


def detect_os_family() -> str:
    system = platform.system()
    if system == "Darwin":
        return "macos"
    if system == "Linux":
        return "linux"
    if system == "Windows":
        build_text = platform.version().split(".")[-1]
        try:
            build = int(build_text)
        except ValueError:
            build = 0
        return "windows11" if build >= 22000 else "windows10"
    return f"unsupported:{system or 'unknown'}"


def host_provenance(graphics_api: str, gpu: str) -> dict:
    family = detect_os_family()
    product = platform.system()
    version = platform.mac_ver()[0] if family == "macos" else platform.release()
    build = platform.version()
    return {
        "os_family": family,
        "os_product": product or "unknown",
        "os_version": version or "unknown",
        "os_build": build or "unknown",
        "kernel": f"{platform.system()} {platform.release()}".strip() or "unknown",
        "architecture": platform.machine() or "unknown",
        "gpu": gpu,
        "graphics_api": graphics_api,
    }


def validate_result(result: dict, cases: dict[str, dict], targets: dict[str, dict]) -> None:
    require(set(result) == RESULT_FIELDS, "result has unknown or missing fields")
    require(result["schema"] == RESULT_SCHEMA, f"result schema must be {RESULT_SCHEMA!r}")
    target = targets.get(result["target"])
    case = cases.get(result["case"])
    require(target is not None, f"result names unknown target {result['target']!r}")
    require(case is not None, f"result names unknown case {result['case']!r}")
    require(result["source_id"] == "git:f0728a2520d5aa735886240de3fee75cc805f6d6", "result source identity drifted")
    host = result["host"]
    require(isinstance(host, dict) and set(host) == HOST_FIELDS, "result host fields are invalid")
    for field in HOST_FIELDS:
        nonempty(host[field], f"result.host.{field}")
    require(host["graphics_api"] == target["graphics_api"], "result graphics API mismatches target")
    require(result["repeat_bar"] == case["repeat_bar"], "result repeat bar mismatches manifest")
    require(isinstance(result["requested_runs"], int) and result["requested_runs"] >= 0, "requested_runs must be nonnegative")
    require(isinstance(result["clean_runs"], int) and 0 <= result["clean_runs"] <= result["requested_runs"], "clean_runs is invalid")
    require(isinstance(result["iterations"], list), "iterations must be an array")
    require(len(result["iterations"]) <= result["requested_runs"], "too many iteration records")
    for index, iteration in enumerate(result["iterations"], 1):
        require(isinstance(iteration, dict) and set(iteration) == {"run", "exit_code"}, f"iteration {index} fields are invalid")
        require(iteration["run"] == index and isinstance(iteration["exit_code"], int), f"iteration {index} is invalid")
    status = result["status"]
    require(status in RESULT_STATUSES, f"invalid result status {status!r}")
    if status == "repeat-bar-passed":
        require(host["os_family"] == target["os_family"], "passing result host mismatches target")
        require(target["case_statuses"][case["id"]] == "pass", "manifest does not admit this case as runnable")
        require(result["clean_runs"] >= case["repeat_bar"], "passing result is below repeat bar")
        require(len(result["iterations"]) == result["requested_runs"] and all(item["exit_code"] == 0 for item in result["iterations"]), "passing result contains failed or missing iterations")
        require(result["reason"] is None, "passing result cannot carry a reason")
    elif status == "diagnostic-only":
        require(host["os_family"] == target["os_family"], "diagnostic result host mismatches target")
        require(0 < result["clean_runs"] < case["repeat_bar"], "diagnostic result must be clean but below repeat bar")
        require(len(result["iterations"]) == result["requested_runs"] and all(item["exit_code"] == 0 for item in result["iterations"]), "diagnostic result contains failed or missing iterations")
        require(result["reason"] is None, "diagnostic result cannot carry a reason")
    elif status in {"blocked", "skipped"}:
        require(result["clean_runs"] == 0 and not result["iterations"], f"{status} result cannot contain executions")
        nonempty(result["reason"], f"{status} reason")
        if status == "blocked":
            require(host["os_family"] == target["os_family"], "blocked result must come from the matching OS family")
            require(target["case_statuses"][case["id"]] == "blocked", "blocked result disagrees with manifest case status")
        else:
            require(host["os_family"] != target["os_family"], "skipped result requires a host/target mismatch")
    else:
        require(result["requested_runs"] > 0 and result["clean_runs"] < result["requested_runs"], "failed result must stop before all runs are clean")
        require(len(result["iterations"]) == result["clean_runs"] + 1, "failed result must retain exactly one failing iteration")
        require(all(item["exit_code"] == 0 for item in result["iterations"][:-1]) and result["iterations"][-1]["exit_code"] != 0, "failed result iteration sequence is invalid")
        nonempty(result["reason"], "failure reason")
    timestamp = nonempty(result["recorded_at_utc"], "recorded_at_utc")
    try:
        parsed_timestamp = dt.datetime.fromisoformat(timestamp)
    except ValueError as error:
        raise CertificationError("recorded_at_utc is not ISO-8601") from error
    require(parsed_timestamp.tzinfo is not None, "recorded_at_utc must include a UTC offset")
    digest = result["result_sha256"]
    require(isinstance(digest, str) and len(digest) == 64, "result SHA-256 is invalid")
    require(digest == hashlib.sha256(canonical_result(result)).hexdigest(), "result SHA-256 mismatch")


def validate_rt64_tree(path: Path, commit: str) -> None:
    require(path.is_absolute(), "--rt64-dir must be absolute")
    require(path.is_dir(), f"RT64 source tree does not exist: {path}")
    try:
        head = subprocess.run(["git", "-C", str(path), "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
        dirty = subprocess.run(["git", "-C", str(path), "status", "--porcelain=v1", "--untracked-files=all"], check=True, capture_output=True, text=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise CertificationError(f"cannot inspect RT64 source tree: {error}") from error
    require(head == commit, f"RT64 HEAD {head!r} does not match pinned {commit!r}")
    dirty_summary = dirty.splitlines()[0] if dirty else "<clean>"
    require(not dirty, f"RT64 source tree is dirty: {dirty_summary!r}")


def write_result(path: Path, result: dict) -> None:
    require(not path.exists(), f"refusing to overwrite retained result {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    require(not temporary.exists(), f"temporary result path already exists: {temporary}")
    temporary.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def make_nonrun_result(manifest: dict, target: dict, case: dict, gpu: str, status: str, reason: str) -> dict:
    return sign_result({
        "schema": RESULT_SCHEMA,
        "target": target["id"],
        "case": case["id"],
        "source_id": manifest["source"]["source_id"],
        "host": host_provenance(target["graphics_api"], gpu),
        "requested_runs": 0,
        "repeat_bar": case["repeat_bar"],
        "clean_runs": 0,
        "iterations": [],
        "status": status,
        "reason": reason,
        "recorded_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "result_sha256": "",
    })


def run_case(manifest: dict, cases: dict[str, dict], targets: dict[str, dict], selection: str, runs: int | None, gpu: str, rt64_dir: Path | None, result_path: Path, root: Path) -> int:
    require(":" in selection, "--run must be TARGET:CASE")
    target_id, case_id = selection.split(":", 1)
    target = targets.get(target_id)
    case = cases.get(case_id)
    require(target is not None, f"unknown target {target_id!r}")
    require(case is not None, f"unknown case {case_id!r}")
    nonempty(gpu, "--gpu")
    actual_family = detect_os_family()
    if actual_family != target["os_family"]:
        result = make_nonrun_result(manifest, target, case, gpu, "skipped", f"host OS {actual_family!r} does not match target {target['os_family']!r}")
        validate_result(result, cases, targets)
        write_result(result_path, result)
        print(json.dumps(result, indent=2))
        return 2
    if target["case_statuses"][case_id] != "pass":
        result = make_nonrun_result(manifest, target, case, gpu, "blocked", target["frontier"])
        validate_result(result, cases, targets)
        write_result(result_path, result)
        print(json.dumps(result, indent=2))
        return 2
    require(rt64_dir is not None, "matching live execution requires --rt64-dir")
    require(runs is None or runs > 0, "--runs must be positive")
    validate_rt64_tree(rt64_dir, manifest["source"]["rt64_commit"])
    count = runs if runs is not None else case["repeat_bar"]
    iterations = []
    clean = 0
    reason = None
    environment = os.environ.copy()
    environment["FN64_RT64_DIR"] = str(rt64_dir)
    for number in range(1, count + 1):
        print(f"rt64-platform-certification: {selection} run {number}/{count}", flush=True)
        try:
            completed = subprocess.run([
                "cargo", "run", "-p", "fn64-render-rt64", "--features", ",".join(case["features"]),
                "--example", case["example"],
            ], cwd=root, env=environment, check=False, timeout=LIVE_CASE_TIMEOUT_SECONDS)
            code = completed.returncode
        except subprocess.TimeoutExpired:
            code = 124
            reason = (
                f"case exceeded the {LIVE_CASE_TIMEOUT_SECONDS}-second "
                "per-process watchdog"
            )
        except OSError as error:
            code = 127
            reason = str(error)
        iterations.append({"run": number, "exit_code": code})
        if code != 0:
            reason = reason or f"cargo exited {code}"
            break
        clean += 1
    if reason is not None:
        status = "failed"
    elif clean < case["repeat_bar"]:
        status = "diagnostic-only"
    else:
        status = "repeat-bar-passed"
    result = sign_result({
        "schema": RESULT_SCHEMA, "target": target_id, "case": case_id,
        "source_id": manifest["source"]["source_id"],
        "host": host_provenance(target["graphics_api"], gpu),
        "requested_runs": count, "repeat_bar": case["repeat_bar"],
        "clean_runs": clean, "iterations": iterations, "status": status,
        "reason": reason, "recorded_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "result_sha256": "",
    })
    validate_result(result, cases, targets)
    write_result(result_path, result)
    print(json.dumps(result, indent=2))
    return 1 if status == "failed" else 0


def selftest(manifest: dict, cases: dict[str, dict], blockers: dict[str, dict], targets: dict[str, dict], legacy: dict, root: Path) -> None:
    shrunk = copy.deepcopy(manifest)
    del shrunk["targets"][0]["case_statuses"]["backend-lifecycle"]
    try:
        validate_manifest(shrunk, root)
    except CertificationError:
        pass
    else:
        raise CertificationError("selftest: shrinking a target case denominator passed")
    shrunk_blockers = copy.deepcopy(manifest)
    shrunk_blockers["targets"][0]["open_blockers"].pop()
    try:
        validate_manifest(shrunk_blockers, root)
    except CertificationError:
        pass
    else:
        raise CertificationError("selftest: shrinking a target blocker denominator passed")
    shrunk_targets = copy.deepcopy(manifest)
    shrunk_targets["targets"].pop()
    try:
        validate_manifest(shrunk_targets, root)
    except CertificationError:
        pass
    else:
        raise CertificationError("selftest: shrinking the target denominator passed")
    result = make_nonrun_result(manifest, targets["linux-vulkan"], cases["backend-lifecycle"], "not-probed", "skipped", "selftest mismatch")
    validate_result(result, cases, targets)
    result["host"]["gpu"] = "tampered"
    try:
        validate_result(result, cases, targets)
    except CertificationError:
        pass
    else:
        raise CertificationError("selftest: tampered result passed integrity")
    require(len(cases) == 13 and len(blockers) == 7 and len(legacy["cases"]) == 11, "selftest fixture denominator drifted")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=Path("docs/rt64-platform-certification.json"))
    parser.add_argument("--doc", type=Path, default=Path("docs/RT64-PLATFORM-CERTIFICATION.md"))
    actions = parser.add_mutually_exclusive_group()
    actions.add_argument("--check", action="store_true")
    actions.add_argument("--write-doc", action="store_true")
    actions.add_argument("--list", action="store_true")
    actions.add_argument("--plan", metavar="TARGET")
    actions.add_argument("--run", metavar="TARGET:CASE")
    actions.add_argument("--verify-result", type=Path)
    actions.add_argument("--selftest", action="store_true")
    parser.add_argument("--runs", type=int)
    parser.add_argument("--gpu")
    parser.add_argument("--rt64-dir", type=Path)
    parser.add_argument("--result", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    try:
        manifest = load_json(args.manifest)
        cases, blockers, targets, legacy = validate_manifest(manifest, root)
        rendered = render_doc(manifest, cases, blockers, targets, legacy)
        if args.write_doc:
            args.doc.write_text(rendered, encoding="utf-8")
            print(f"rt64-platform-certification: wrote {args.doc}")
            return 0
        if args.list:
            for target in targets.values():
                counts = {state: list(target["case_statuses"].values()).count(state) for state in ("pass", "blocked", "skipped")}
                print(f"{target['id']}\t{target['graphics_api']}\tpass={counts['pass']}\tblocked={counts['blocked']}\tskipped={counts['skipped']}")
            return 0
        if args.plan:
            target = targets.get(args.plan)
            require(target is not None, f"unknown target {args.plan!r}")
            for case in cases.values():
                print(f"{case['id']}\t{case['repeat_bar']}\t{target['case_statuses'][case['id']]}\t{case['example']}")
            return 0
        if args.verify_result:
            validate_result(load_json(args.verify_result), cases, targets)
            print(f"rt64-platform-certification: valid result {args.verify_result}")
            return 0
        if args.selftest:
            selftest(manifest, cases, blockers, targets, legacy, root)
            print("rt64-platform-certification: selftest passed")
            return 0
        if args.run:
            require(args.result is not None, "--run requires --result so evidence is retained")
            return run_case(manifest, cases, targets, args.run, args.runs, args.gpu or "", args.rt64_dir, args.result, root)
        require(args.runs is None and args.gpu is None and args.rt64_dir is None and args.result is None, "execution options require --run")
        checked = args.doc.read_text(encoding="utf-8")
        require(checked == rendered, f"generated doc is stale: {args.doc}; run --write-doc")
    except (CertificationError, OSError) as error:
        print(f"rt64-platform-certification: {error}", file=sys.stderr)
        return 1
    print(f"rt64-platform-certification: clean ({len(targets)} targets, {len(cases)} cases each, {len(blockers)} blockers each; all platform rows open)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
