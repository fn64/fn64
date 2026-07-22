#!/usr/bin/env python3
"""Validate, render, and run fn64's macOS RT64 certification manifest."""

from __future__ import annotations

import argparse
import json
import os
import platform
import subprocess
import sys
import time
from pathlib import Path


SCHEMA = "fn64.rt64-macos-certification.v1"
LIVE_CASE_TIMEOUT_SECONDS = 60
LIVE_CASE_TEARDOWN_SECONDS = 10.0
EXPECTED_CASES = {
    "backend-lifecycle": "backend",
    "resolution-downsample": "resolution",
    "framebuffer-rdram-region": "framebuffer",
    "framebuffer-enhancement": "framebuffer",
    "texture-replacements": "textures",
    "latency-skip-buffering": "latency",
    "latency-present-early": "latency",
    "deferred-debugger": "inspection",
    "ubershader-critical-path": "pipelines",
    "hfr-hle-cooperation": "generated-frames",
    "extended-gbi-cooperation": "extended-gbi",
}
EXPECTED_BLOCKERS = {
    "recognized-hle-and-extended-gbi",
    "aspect-and-generated-frames",
    "remaining-user-controls",
    "remaining-enhancement-controls",
    "metal-inspector-gui",
    "full-adapter-rom-coverage",
    "declared-host-range",
}
ROOT_FIELDS = {
    "schema",
    "platform_claim",
    "source",
    "recorded_host",
    "denominator",
    "cases",
    "blockers",
}
SOURCE_FIELDS = {"rt64_commit", "source_id", "provenance", "post_vi_api"}
HOST_FIELDS = {"product", "version", "build", "kernel", "architecture", "gpu"}
CASE_FIELDS = {"id", "category", "example", "features", "repeat_bar", "claims", "recorded"}
RECORDED_FIELDS = {"status", "clean_runs", "verified_on"}
BLOCKER_FIELDS = {"id", "status", "claims", "reason"}


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


def nonempty_string(value: object, where: str) -> str:
    require(isinstance(value, str) and bool(value.strip()), f"{where} must be nonempty")
    return value


def string_list(value: object, where: str) -> list[str]:
    require(isinstance(value, list), f"{where} must be an array")
    require(
        all(isinstance(item, str) and item for item in value),
        f"{where} entries must be nonempty strings",
    )
    require(len(value) == len(set(value)), f"{where} contains duplicates")
    return value


def inventory_by_id(root: Path) -> dict[str, dict]:
    inventory = load_json(root / "docs/rt64-public-feature-inventory.json")
    items = inventory.get("items")
    require(isinstance(items, list), "public feature inventory items must be an array")
    by_id = {item.get("id"): item for item in items if isinstance(item, dict)}
    require(len(by_id) == len(items), "public feature inventory contains duplicate or invalid IDs")
    return by_id


def validate_manifest(manifest: dict, root: Path) -> tuple[list[dict], list[dict]]:
    require(set(manifest) == ROOT_FIELDS, "unknown or missing manifest root field")
    require(manifest["schema"] == SCHEMA, f"schema must be {SCHEMA!r}")
    require(manifest["platform_claim"] == "platform-macos", "unexpected platform claim")
    nonempty_string(manifest["denominator"], "denominator")

    source = manifest["source"]
    require(isinstance(source, dict) and set(source) == SOURCE_FIELDS, "invalid source fields")
    commit = nonempty_string(source["rt64_commit"], "source.rt64_commit")
    require(len(commit) == 40 and all(char in "0123456789abcdef" for char in commit), "source.rt64_commit must be a lowercase full Git commit")
    require(source["source_id"] == f"git:{commit}", "source.source_id must name the pinned commit")
    require(source["provenance"] == "GitClean", "source.provenance must be GitClean")
    require(source["post_vi_api"] == "metal-bgra8-unorm", "unexpected post-VI API")

    host = manifest["recorded_host"]
    require(isinstance(host, dict) and set(host) == HOST_FIELDS, "invalid recorded_host fields")
    for field in HOST_FIELDS:
        nonempty_string(host[field], f"recorded_host.{field}")
    require(host["product"] == "macOS", "recorded host must be macOS")

    cargo_toml = (root / "crates/fn64-certification/Cargo.toml").read_text(encoding="utf-8")
    inventory = inventory_by_id(root)
    cases = manifest["cases"]
    require(isinstance(cases, list), "cases must be an array")
    case_ids: set[str] = set()
    for index, case in enumerate(cases):
        where = f"cases[{index}]"
        require(isinstance(case, dict) and set(case) == CASE_FIELDS, f"{where}: invalid fields")
        case_id = nonempty_string(case["id"], f"{where}.id")
        require(case_id not in case_ids, f"duplicate case ID {case_id!r}")
        case_ids.add(case_id)
        require(
            EXPECTED_CASES.get(case_id) == case["category"],
            f"{case_id}: missing or incorrect required category",
        )
        example = nonempty_string(case["example"], f"{case_id}.example")
        example_path = root / f"crates/fn64-certification/examples/{example}.rs"
        require(example_path.is_file(), f"{case_id}: missing example {example_path.relative_to(root)}")
        require(f'name = "{example}"' in cargo_toml, f"{case_id}: example missing from Cargo.toml")
        source_text = example_path.read_text(encoding="utf-8")
        require(source["source_id"] in source_text, f"{case_id}: example does not enforce pinned source identity")
        features = string_list(case["features"], f"{case_id}.features")
        require(features, f"{case_id}: features must not be empty")
        repeat_bar = case["repeat_bar"]
        require(repeat_bar in {10, 20}, f"{case_id}: repeat bar must be 10 or 20")
        claims = string_list(case["claims"], f"{case_id}.claims")
        require(claims, f"{case_id}: claims must not be empty")
        for claim_id in claims:
            require(claim_id in inventory, f"{case_id}: unknown claim {claim_id!r}")
            require(inventory[claim_id].get("status") == "closed", f"{case_id}: claim {claim_id!r} is not closed")
        recorded = case["recorded"]
        require(isinstance(recorded, dict) and set(recorded) == RECORDED_FIELDS, f"{case_id}.recorded: invalid fields")
        require(recorded["status"] == "pass", f"{case_id}: recorded status must be pass")
        require(isinstance(recorded["clean_runs"], int), f"{case_id}: clean_runs must be an integer")
        require(recorded["clean_runs"] >= repeat_bar, f"{case_id}: recorded runs are below the repeat bar")
        nonempty_string(recorded["verified_on"], f"{case_id}.recorded.verified_on")
    require(case_ids == set(EXPECTED_CASES), f"case denominator drifted: missing={sorted(set(EXPECTED_CASES) - case_ids)}, extra={sorted(case_ids - set(EXPECTED_CASES))}")

    blockers = manifest["blockers"]
    require(isinstance(blockers, list), "blockers must be an array")
    blocker_ids: set[str] = set()
    for index, blocker in enumerate(blockers):
        where = f"blockers[{index}]"
        require(isinstance(blocker, dict) and set(blocker) == BLOCKER_FIELDS, f"{where}: invalid fields")
        blocker_id = nonempty_string(blocker["id"], f"{where}.id")
        require(blocker_id not in blocker_ids, f"duplicate blocker ID {blocker_id!r}")
        blocker_ids.add(blocker_id)
        require(blocker["status"] == "open", f"{blocker_id}: blocker status must be open")
        nonempty_string(blocker["reason"], f"{blocker_id}.reason")
        for claim_id in string_list(blocker["claims"], f"{blocker_id}.claims"):
            require(claim_id in inventory, f"{blocker_id}: unknown claim {claim_id!r}")
            require(inventory[claim_id].get("status") == "open", f"{blocker_id}: claim {claim_id!r} is not open")
    require(blocker_ids == EXPECTED_BLOCKERS, f"blocker denominator drifted: missing={sorted(EXPECTED_BLOCKERS - blocker_ids)}, extra={sorted(blocker_ids - EXPECTED_BLOCKERS)}")
    platform_claim = inventory.get(manifest["platform_claim"])
    require(platform_claim is not None, "platform-macos is absent from the public inventory")
    require(platform_claim.get("status") == "open", "platform-macos must remain open while certification blockers remain")
    return cases, blockers


def render_doc(manifest: dict, cases: list[dict], blockers: list[dict]) -> str:
    source = manifest["source"]
    host = manifest["recorded_host"]
    lines = [
        "# RT64 macOS certification",
        "",
        "This is generated from `docs/rt64-macos-certification.json`. Edit the JSON and",
        "run `python3 tools/rt64_macos_certification.py --write-doc`.",
        "",
        "## Status",
        "",
        "**The platform-wide `platform-macos` claim remains open.** The cases below",
        "are closed feature-specific evidence, not a substitute for the unresolved",
        "adapter, enhancement, GUI, full-ROM, or host-range denominator.",
        "",
        f"Denominator: {manifest['denominator']}",
        "",
        f"Pinned RT64: `{source['source_id']}` (`{source['provenance']}`); capture API: `{source['post_vi_api']}`.",
        "",
        f"Recorded host: {host['product']} {host['version']} build {host['build']}; {host['kernel']} {host['architecture']}; {host['gpu']}.",
        "",
        "## Feature-specific live Metal cases",
        "",
        "| Case | Category | Example | Repeat bar | Recorded result | Closed claims |",
        "|---|---|---|---:|---|---|",
    ]
    for case in cases:
        claims = ", ".join(f"`{claim}`" for claim in case["claims"])
        recorded = case["recorded"]
        example = case["example"]
        example_link = f"[\u200b`{example}`](../crates/fn64-certification/examples/{example}.rs)"
        lines.append(
            f"| `{case['id']}` | {case['category']} | {example_link} | {case['repeat_bar']} | "
            f"{recorded['clean_runs']} clean ({recorded['verified_on']}) | {claims} |"
        )
    lines.extend(
        [
            "",
            "A manifest result is record evidence only when its run count meets the case's",
            "repeat bar. A shorter runner invocation is labeled `diagnostic-only` even when",
            "every invocation exits successfully. Unavailable and skipped cases are errors.",
            "",
            "## Open platform denominator",
            "",
            "| Blocker | Related open claims | Exact frontier |",
            "|---|---|---|",
        ]
    )
    for blocker in blockers:
        claims = ", ".join(f"`{claim}`" for claim in blocker["claims"]) or "—"
        lines.append(f"| `{blocker['id']}` | {claims} | {blocker['reason']} |")
    lines.extend(
        [
            "",
            "## Validation and execution",
            "",
            "```sh",
            "python3 tools/rt64_macos_certification.py --check",
            "python3 tools/rt64_macos_certification.py --list",
            "python3 tools/rt64_macos_certification.py --run backend-lifecycle",
            "python3 tools/rt64_macos_certification.py --run backend-lifecycle --runs 1",
            "```",
            "",
            "The first run command uses the manifest repeat bar. The second is deliberately",
            "diagnostic-only. `--run all` executes every case at its own repeat bar unless",
            "`--runs` supplies a common diagnostic or repeat count. Execution requires",
            "Darwin and an exact, clean pinned RT64 source tree selected by `FN64_RT64_DIR`",
            "or the default sibling checkout. Every live case invocation fails if it exceeds",
            f"the shared {LIVE_CASE_TIMEOUT_SECONDS}-second per-process watchdog. Fresh",
            f"processes are spaced by {LIVE_CASE_TEARDOWN_SECONDS:g} seconds so WindowServer",
            "can reclaim each hidden Metal surface before the next case invocation.",
            "",
        ]
    )
    return "\n".join(lines)


def validate_rt64_tree(rt64_dir: Path, commit: str) -> None:
    require(rt64_dir.is_dir(), f"RT64 source tree does not exist: {rt64_dir}")
    try:
        head = subprocess.run(
            ["git", "-C", str(rt64_dir), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        dirty = subprocess.run(
            ["git", "-C", str(rt64_dir), "status", "--porcelain=v1", "--untracked-files=all"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise CertificationError(f"cannot inspect RT64 source tree {rt64_dir}: {error}") from error
    require(head == commit, f"RT64 HEAD {head!r} does not match pinned commit {commit!r}")
    dirty_summary = dirty.splitlines()[0] if dirty else "<clean>"
    require(not dirty, f"RT64 source tree must be clean; status begins {dirty_summary!r}")


def run_cases(manifest: dict, cases: list[dict], selection: str, requested_runs: int | None, root: Path) -> int:
    require(sys.platform == "darwin", "live certification requires Darwin; unavailable cases are not passes")
    require(requested_runs is None or requested_runs > 0, "--runs must be positive")
    selected = cases if selection == "all" else [case for case in cases if case["id"] == selection]
    require(selected, f"unknown case {selection!r}")
    configured = os.environ.get("FN64_RT64_DIR")
    rt64_dir = Path(configured).expanduser().resolve() if configured else (root.parent / "no-mercy-recompiled/third_party/rt64").resolve()
    validate_rt64_tree(rt64_dir, manifest["source"]["rt64_commit"])
    results: list[dict] = []
    exit_code = 0
    environment = os.environ.copy()
    environment["FN64_RT64_DIR"] = str(rt64_dir)
    for case in selected:
        count = requested_runs if requested_runs is not None else case["repeat_bar"]
        clean_runs = 0
        failure: str | None = None
        for run_number in range(1, count + 1):
            print(f"macos-certification: {case['id']} run {run_number}/{count}", flush=True)
            try:
                completed = subprocess.run(
                    [
                        "cargo",
                        "run",
                        "-p",
                        "fn64-certification",
                        "--features",
                        ",".join(case["features"]),
                        "--example",
                        case["example"],
                    ],
                    cwd=root,
                    env=environment,
                    check=False,
                    timeout=LIVE_CASE_TIMEOUT_SECONDS,
                )
            except subprocess.TimeoutExpired:
                failure = (
                    f"case exceeded the {LIVE_CASE_TIMEOUT_SECONDS}-second "
                    "per-process watchdog"
                )
                break
            except OSError as error:
                failure = str(error)
                break
            if completed.returncode != 0:
                failure = f"cargo exited {completed.returncode}"
                break
            clean_runs += 1
            if run_number != count:
                # Interleaving closed here: the child exits while WindowServer
                # is still reclaiming its hidden Metal surface, then the next
                # child asks CoreGraphics for a display-service connection and
                # can stall behind that reclamation.
                time.sleep(LIVE_CASE_TEARDOWN_SECONDS)
        if failure is not None:
            status = "failed"
            exit_code = 1
        elif count < case["repeat_bar"]:
            status = "diagnostic-only"
        else:
            status = "repeat-bar-passed"
        results.append(
            {
                "case": case["id"],
                "status": status,
                "clean_runs": clean_runs,
                "requested_runs": count,
                "repeat_bar": case["repeat_bar"],
                "failure": failure,
            }
        )
    report = {
        "schema": "fn64.rt64-macos-certification-run.v1",
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
        },
        "source_id": manifest["source"]["source_id"],
        "results": results,
    }
    print(json.dumps(report, indent=2))
    return exit_code


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=Path("docs/rt64-macos-certification.json"))
    parser.add_argument("--doc", type=Path, default=Path("docs/RT64-MACOS-CERTIFICATION.md"))
    actions = parser.add_mutually_exclusive_group()
    actions.add_argument("--check", action="store_true")
    actions.add_argument("--write-doc", action="store_true")
    actions.add_argument("--list", action="store_true")
    actions.add_argument("--run", metavar="CASE")
    parser.add_argument("--runs", type=int)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    try:
        manifest = load_json(args.manifest)
        cases, blockers = validate_manifest(manifest, root)
        rendered = render_doc(manifest, cases, blockers)
        if args.write_doc:
            args.doc.write_text(rendered, encoding="utf-8")
            print(f"rt64-macos-certification: wrote {args.doc}")
            return 0
        if args.list:
            for case in cases:
                print(f"{case['id']}\t{case['repeat_bar']}\t{case['example']}")
            return 0
        if args.run is not None:
            return run_cases(manifest, cases, args.run, args.runs, root)
        require(args.runs is None, "--runs requires --run")
        try:
            checked = args.doc.read_text(encoding="utf-8")
        except OSError as error:
            raise CertificationError(f"cannot read generated doc {args.doc}: {error}") from error
        require(checked == rendered, f"generated doc is stale: {args.doc}; run --write-doc")
    except (CertificationError, OSError) as error:
        print(f"rt64-macos-certification: {error}", file=sys.stderr)
        return 1
    print(
        "rt64-macos-certification: clean "
        f"({len(cases)} feature-specific cases; {len(blockers)} open platform blockers)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
