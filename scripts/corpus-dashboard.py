#!/usr/bin/env python3
"""Validate fn64 corpus-stage receipts and render a content-free dashboard.

The private campaign directory is the evidence store. This program never reads
ROM bytes or artifact bodies; it validates receipt metadata and derives a
current stage matrix. It deliberately rejects ambiguous winners and broken
predecessor chains rather than presenting a friendlier but false dashboard.
"""

from __future__ import annotations

import argparse
import hashlib
import html
import json
import os
import re
import secrets
import stat
import sys
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any


MANIFEST_SCHEMA = "fn64.corpus-campaign.v1"
RECEIPT_SCHEMA = "fn64.corpus-stage-receipt.v1"
DASHBOARD_SCHEMA = "fn64.corpus-dashboard.v1"
STAGES = ("discover", "pack", "recompile", "boot", "interactive", "playtest_ready", "fidelity")
OUTCOMES = frozenset(("passed", "frontier", "resource_limit", "invalid_input", "infrastructure_failure"))
SHA256 = re.compile(r"^[0-9a-f]{64}$")


class DashboardError(Exception):
    """A malformed campaign is evidence failure, never a partial dashboard."""


def canonical_json(value: Any) -> bytes:
    try:
        return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
    except (TypeError, ValueError) as error:
        raise DashboardError("value is not canonical JSON") from error


def require_sha(value: Any, field: str) -> str:
    if not isinstance(value, str) or not SHA256.fullmatch(value):
        raise DashboardError(f"{field} is not a lowercase SHA-256")
    return value


def read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise DashboardError(f"cannot read {path}: {error}") from error
    if not isinstance(value, dict):
        raise DashboardError(f"{path} is not a JSON object")
    return value


def load_manifest(path: Path) -> dict[str, Any]:
    manifest = read_json(path)
    if manifest.get("schema") != MANIFEST_SCHEMA:
        raise DashboardError(f"manifest is not {MANIFEST_SCHEMA}")
    if not isinstance(manifest.get("campaign_id"), str) or not manifest["campaign_id"]:
        raise DashboardError("manifest campaign_id is missing")
    roms = manifest.get("roms")
    if not isinstance(roms, list) or not roms:
        raise DashboardError("manifest roms is empty")
    seen = set()
    for rom in roms:
        if not isinstance(rom, dict) or not isinstance(rom.get("id"), str):
            raise DashboardError("manifest ROM has no string id")
        if rom["id"] in seen:
            raise DashboardError(f"manifest repeats ROM id {rom['id']}")
        seen.add(rom["id"])
        require_sha(rom.get("normalized_sha256"), f"manifest ROM {rom['id']}")
    return manifest


def validate_receipt(value: dict[str, Any], manifest: dict[str, Any], source: Path) -> dict[str, Any]:
    if value.get("schema") != RECEIPT_SCHEMA:
        raise DashboardError(f"{source}: wrong receipt schema")
    if value.get("campaign_id") != manifest["campaign_id"]:
        raise DashboardError(f"{source}: wrong campaign")
    for name in ("receipt_id", "attempt_id", "finished_at"):
        if not isinstance(value.get(name), str) or not value[name]:
            raise DashboardError(f"{source}: missing {name}")
    stage = value.get("stage")
    if stage not in STAGES:
        raise DashboardError(f"{source}: unknown stage {stage!r}")
    rom = value.get("rom")
    if not isinstance(rom, dict) or not isinstance(rom.get("id"), str):
        raise DashboardError(f"{source}: malformed rom")
    manifest_roms = {item["id"]: item["normalized_sha256"] for item in manifest["roms"]}
    if manifest_roms.get(rom["id"]) != require_sha(rom.get("normalized_sha256"), f"{source}: ROM digest"):
        raise DashboardError(f"{source}: ROM is absent or does not match manifest")
    outcome = value.get("outcome")
    if not isinstance(outcome, dict) or outcome.get("kind") not in OUTCOMES:
        raise DashboardError(f"{source}: malformed outcome")
    if outcome["kind"] == "frontier":
        frontier = outcome.get("frontier")
        if not isinstance(frontier, dict) or not isinstance(frontier.get("kind"), str):
            raise DashboardError(f"{source}: frontier has no stable kind")
    elif outcome.get("frontier") is not None:
        raise DashboardError(f"{source}: non-frontier outcome carries frontier")
    predecessors = value.get("predecessors")
    if not isinstance(predecessors, list) or not all(isinstance(item, str) and item for item in predecessors):
        raise DashboardError(f"{source}: malformed predecessors")
    if len(set(predecessors)) != len(predecessors):
        raise DashboardError(f"{source}: duplicate predecessor")
    if not isinstance(value.get("result"), dict):
        raise DashboardError(f"{source}: result is not an object")
    return value


def load_receipts(receipt_root: Path, manifest: dict[str, Any]) -> list[dict[str, Any]]:
    if not receipt_root.is_dir():
        raise DashboardError("receipt root is not a directory")
    receipts = []
    ids = set()
    for path in sorted(receipt_root.rglob("*.json")):
        receipt = validate_receipt(read_json(path), manifest, path)
        if receipt["receipt_id"] in ids:
            raise DashboardError(f"duplicate receipt_id {receipt['receipt_id']}")
        ids.add(receipt["receipt_id"])
        receipts.append(receipt)
    if not receipts:
        raise DashboardError("receipt root contains no receipts")
    return receipts


def derive(manifest: dict[str, Any], receipts: list[dict[str, Any]]) -> dict[str, Any]:
    """Choose newest valid evidence per stage and derive contiguous progress."""
    by_key: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    by_id = {receipt["receipt_id"]: receipt for receipt in receipts}
    for receipt in receipts:
        by_key[(receipt["rom"]["id"], receipt["stage"])].append(receipt)
    rows = []
    frontier_counts: Counter[str] = Counter()
    outcome_counts: Counter[str] = Counter()
    stage_pass_counts: Counter[str] = Counter()
    for rom in manifest["roms"]:
        selected: dict[str, dict[str, Any]] = {}
        history: dict[str, int] = {}
        for stage in STAGES:
            candidates = by_key.get((rom["id"], stage), [])
            history[stage] = len(candidates)
            if not candidates:
                continue
            newest = max(item["finished_at"] for item in candidates)
            winners = [item for item in candidates if item["finished_at"] == newest]
            if len(winners) != 1:
                raise DashboardError(f"{rom['id']} {stage}: ambiguous newest receipt")
            selected[stage] = winners[0]
        highest = "not_run"
        blocked_at = None
        blocker_kind = None
        next_stage = "discover"
        for index, stage in enumerate(STAGES):
            receipt = selected.get(stage)
            if receipt is None:
                next_stage = stage
                break
            required = STAGES[index - 1] if index else None
            if required:
                predecessor = selected.get(required)
                if predecessor is None or predecessor["receipt_id"] not in receipt["predecessors"]:
                    raise DashboardError(f"{rom['id']} {stage}: selected receipt does not bind selected {required}")
            kind = receipt["outcome"]["kind"]
            outcome_counts[kind] += 1
            if kind != "passed":
                blocked_at = stage
                if kind == "frontier":
                    blocker_kind = receipt["outcome"]["frontier"]["kind"]
                    frontier_counts[blocker_kind] += 1
                else:
                    blocker_kind = kind
                break
            highest = stage
            stage_pass_counts[stage] += 1
            next_stage = STAGES[index + 1] if index + 1 < len(STAGES) else None
        rows.append({
            "rom": rom,
            "highest_passed_stage": highest,
            "blocked_at": blocked_at,
            "blocker_kind": blocker_kind,
            "next_stage": next_stage,
            "status": f"blocked_at_{blocked_at}" if blocked_at else f"awaiting_{next_stage}" if next_stage else "complete",
            "selected_receipts": {stage: receipt["receipt_id"] for stage, receipt in selected.items()},
            "attempt_counts": history,
        })
    return {
        "schema": DASHBOARD_SCHEMA,
        "campaign_id": manifest["campaign_id"],
        "rom_count": len(rows),
        "rows": rows,
        "stage_pass_counts": {stage: stage_pass_counts[stage] for stage in STAGES},
        "frontier_counts": dict(sorted(frontier_counts.items())),
        "selected_outcome_counts": dict(sorted(outcome_counts.items())),
    }


def write_new(path: Path, content: bytes) -> None:
    if not path.is_absolute() or ".." in path.parts or path.exists():
        raise DashboardError("output must be an absolute new path without '..'")
    parent = path.parent
    if not parent.is_dir() or parent.resolve(strict=True) != parent:
        raise DashboardError("output parent must be an existing canonical directory")
    temporary = parent / f".{path.name}.tmp-{os.getpid()}-{secrets.token_hex(8)}"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, stat.S_IRUSR | stat.S_IWUSR)
    try:
        os.write(descriptor, content)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    try:
        os.link(temporary, path)
    except FileExistsError as error:
        raise DashboardError("refusing to overwrite output") from error
    finally:
        os.unlink(temporary)


def render_html(report: dict[str, Any]) -> bytes:
    count = report["rom_count"]
    stage_cards = "".join(
        "<li><strong>{}</strong><span>{} / {}</span></li>".format(
            html.escape(stage.replace("_", " ")), report["stage_pass_counts"][stage], count)
        for stage in STAGES
    )
    frontier_rows = "".join(
        "<li><code>{}</code><span>{}</span></li>".format(html.escape(kind), value)
        for kind, value in report["frontier_counts"].items()
    ) or "<li><span>No current typed frontiers</span></li>"
    rows = "".join(
        "<tr><td>{}</td><td><span class=stage>{}</span></td><td>{}</td><td><code>{}</code></td></tr>".format(
            html.escape(row["rom"]["id"]), html.escape(row["highest_passed_stage"]),
            html.escape(row["blocked_at"] or "awaiting " + (row["next_stage"] or "—")), html.escape(row["blocker_kind"] or "—"))
        for row in report["rows"]
    )
    return """<!doctype html>
<meta charset=utf-8><title>fn64 corpus dashboard</title>
<style>
body{{background:#0b1020;color:#e5e7eb;font:15px/1.45 system-ui,sans-serif;margin:0;padding:40px;max-width:1100px}}
h1{{margin:0;color:#fff}} .subtitle{{color:#94a3b8;margin-top:4px}} h2{{font-size:16px;margin:32px 0 10px;color:#cbd5e1}}
.grid{{display:grid;grid-template-columns:2fr 1fr;gap:20px}}.panel{{background:#121a2e;border:1px solid #24314d;border-radius:10px;padding:18px}}
ul{{list-style:none;padding:0;margin:0;display:grid;grid-template-columns:repeat(2,1fr);gap:8px}}li{{display:flex;justify-content:space-between;gap:12px;color:#cbd5e1}}li span{{color:#f8fafc}}
table{{border-collapse:collapse;width:100%;background:#121a2e;border:1px solid #24314d;border-radius:10px;overflow:hidden}}th,td{{padding:11px 13px;text-align:left;border-bottom:1px solid #24314d}}th{{color:#94a3b8;font-size:12px;text-transform:uppercase;letter-spacing:.05em}}tr:last-child td{{border:0}}.stage{{background:#123b34;color:#a7f3d0;padding:3px 8px;border-radius:99px}}code{{color:#fbbf24}}
</style>
<h1>fn64 corpus dashboard</h1><p class=subtitle>Campaign: {campaign} · {count} ROM images · receipts are the authority</p>
<div class=grid><section class=panel><h2>Stage progress</h2><ul>{stage_cards}</ul></section><section class=panel><h2>Current frontiers</h2><ul>{frontiers}</ul></section></div>
<h2>Per-ROM status</h2><table><thead><tr><th>ROM</th><th>highest passed stage</th><th>next state</th><th>typed blocker</th></tr></thead><tbody>{rows}</tbody></table>
""".format(campaign=html.escape(report["campaign_id"]), count=count, stage_cards=stage_cards, frontiers=frontier_rows, rows=rows).encode()


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--receipts", required=True, type=Path)
    parser.add_argument("--output-json", required=True, type=Path)
    parser.add_argument("--output-html", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        manifest = load_manifest(args.manifest)
        report = derive(manifest, load_receipts(args.receipts, manifest))
        write_new(args.output_json, canonical_json(report) + b"\n")
        write_new(args.output_html, render_html(report))
    except DashboardError as error:
        print(f"corpus-dashboard: FAILED: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
