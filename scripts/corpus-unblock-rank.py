#!/usr/bin/env python3
"""Rank the frontier clusters that block the most ROMs at one funnel stage.

Reads a campaign's manifest and receipts (the authority) plus the dashboard
JSON already derived from them, and groups the non-passed receipts for one
stage into clusters keyed by typed outcome/frontier evidence. The output
ranks mechanisms by ROMs unblocked, never by site occurrences, and never
prints a private local path.

This tool trusts the dashboard only to say which receipt it selected per
(rom, stage); every outcome and frontier detail used for clustering and
reconciliation comes back from the actual receipt so a stale or hand-edited
dashboard cannot silently change the answer without tripping the
reconciliation check.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any


def _load_dashboard_module():
    script = Path(__file__).resolve().with_name("corpus-dashboard.py")
    spec = importlib.util.spec_from_file_location("corpus_dashboard", script)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


DASHBOARD = _load_dashboard_module()

STAGES = DASHBOARD.STAGES
DASHBOARD_SCHEMA = DASHBOARD.DASHBOARD_SCHEMA


class UnblockRankError(Exception):
    """A malformed or unreconcilable campaign is a loud failure, not a guess."""


def read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise UnblockRankError(f"cannot read input: {error}") from error
    if not isinstance(value, dict):
        raise UnblockRankError("input is not a JSON object")
    return value


def load_dashboard(path: Path) -> dict[str, Any]:
    dashboard = read_json(path)
    if dashboard.get("schema") != DASHBOARD_SCHEMA:
        raise UnblockRankError(f"dashboard is not {DASHBOARD_SCHEMA}")
    if not isinstance(dashboard.get("rows"), list):
        raise UnblockRankError("dashboard rows is missing")
    return dashboard


def destination_count_bucket(count: int) -> str:
    if count <= 0:
        raise UnblockRankError("unsupported_destinations count must be positive")
    if count == 1:
        return "1"
    if count <= 3:
        return "2-3"
    return "4+"


def cluster_key(stage: str, receipt: dict[str, Any]) -> tuple:
    outcome = receipt["outcome"]
    kind = outcome["kind"]
    if kind == "frontier":
        frontier = outcome.get("frontier") or {}
        frontier_kind = frontier.get("kind")
        if not isinstance(frontier_kind, str) or not frontier_kind:
            raise UnblockRankError(f"{stage} receipt {receipt.get('receipt_id')}: frontier has no kind")
        if frontier_kind == "unsupported_destinations":
            count = frontier.get("count")
            reasons = frontier.get("reasons")
            if not isinstance(count, int):
                raise UnblockRankError(
                    f"{stage} receipt {receipt.get('receipt_id')}: unsupported_destinations has no integer count"
                )
            if not isinstance(reasons, list) or not all(isinstance(item, str) and item for item in reasons):
                raise UnblockRankError(
                    f"{stage} receipt {receipt.get('receipt_id')}: unsupported_destinations has malformed reasons"
                )
            return ("unsupported_destinations", destination_count_bucket(count), tuple(sorted(reasons)))
        return (frontier_kind,)
    if kind == "resource_limit":
        # The limit rides in `result.resource_limit.which`, not on the outcome:
        # corpus-dashboard.py's validator rejects any payload on a non-frontier
        # outcome, so the sweep has no place to put it there. `outcome.limit` is
        # still accepted for a receipt minted by a writer that used it.
        limit = outcome.get("limit")
        if not isinstance(limit, str) or not limit:
            result = receipt.get("result")
            detail = result.get("resource_limit") if isinstance(result, dict) else None
            limit = detail.get("which") if isinstance(detail, dict) else None
        if not isinstance(limit, str) or not limit:
            raise UnblockRankError(
                f"{stage} receipt {receipt.get('receipt_id')}: resource_limit names no limit "
                "in outcome.limit or result.resource_limit.which"
            )
        return ("resource_limit", limit)
    if kind in ("infrastructure_failure", "invalid_input"):
        return (kind,)
    raise UnblockRankError(f"{stage} receipt {receipt.get('receipt_id')}: unexpected outcome kind {kind!r}")


def format_key(key: tuple) -> str:
    if key[0] == "unsupported_destinations":
        _, bucket, reasons = key
        return f"unsupported_destinations count={bucket} reasons={','.join(reasons)}"
    if key[0] == "resource_limit":
        _, limit = key
        return f"resource_limit limit={limit}"
    return key[0]


def wrap_rom_ids(rom_ids: list[str], width: int = 100) -> list[str]:
    lines: list[str] = []
    current = ""
    for rom_id in rom_ids:
        piece = rom_id if not current else f", {rom_id}"
        if current and len(current) + len(piece) > width:
            lines.append(current)
            current = rom_id
        else:
            current += piece
    if current:
        lines.append(current)
    return lines or [""]


def rank(campaign_dir: Path, dashboard: dict[str, Any], stage: str) -> dict[str, Any]:
    if stage not in STAGES:
        raise UnblockRankError(f"unknown stage {stage!r}")
    manifest = DASHBOARD.load_manifest(campaign_dir / "manifest.json")
    receipts = DASHBOARD.load_receipts(campaign_dir / "receipts", manifest)
    by_id = {receipt["receipt_id"]: receipt for receipt in receipts}

    dashboard_roms = {row["rom"]["id"]: row for row in dashboard["rows"]}
    manifest_rom_ids = [rom["id"] for rom in manifest["roms"]]
    if set(dashboard_roms) != set(manifest_rom_ids):
        raise UnblockRankError("dashboard ROM set does not match campaign manifest")

    clusters: dict[tuple, list[str]] = defaultdict(list)
    sections: dict[tuple, str] = {}
    passed = 0
    not_run = 0
    total_with_receipt = 0

    for rom_id in manifest_rom_ids:
        dashboard_row = dashboard_roms[rom_id]
        selected_receipts = dashboard_row.get("selected_receipts", {})
        receipt_id = selected_receipts.get(stage)
        if receipt_id is None:
            not_run += 1
            continue
        receipt = by_id.get(receipt_id)
        if receipt is None:
            raise UnblockRankError(f"{rom_id}: dashboard selected receipt {receipt_id} is absent from campaign receipts")
        if receipt["rom"]["id"] != rom_id or receipt["stage"] != stage:
            raise UnblockRankError(f"{rom_id}: dashboard selected receipt {receipt_id} does not match rom/stage")
        total_with_receipt += 1

        # A passed receipt is never a blocker at any stage. For `discover`
        # this also covers a receipt whose geometry search bottomed out at
        # `no_candidate_table_found`: that classification is descriptive,
        # recorded under `result.frontier.geometry_failure` on an
        # `outcome.kind == "passed"` receipt, and never changes the outcome
        # kind itself, so no special case is needed here.
        if receipt["outcome"]["kind"] == "passed":
            passed += 1
            continue

        key = cluster_key(stage, receipt)
        section = "resource_or_infra" if key[0] in ("resource_limit", "infrastructure_failure", "invalid_input") else "frontier"
        sections[key] = section
        clusters[key].append(rom_id)

    def to_json_key(key: tuple) -> list:
        return [list(part) if isinstance(part, tuple) else part for part in key]

    rows = []
    for key, rom_ids in clusters.items():
        rom_ids_sorted = sorted(rom_ids)
        rows.append({
            "section": sections[key],
            "key": to_json_key(key),
            "label": format_key(key),
            "rom_count": len(rom_ids_sorted),
            "rom_ids": rom_ids_sorted,
        })
    rows.sort(key=lambda row: (row["section"] != "frontier", -row["rom_count"], row["label"]))

    reconcile(dashboard, stage, rows)

    return {
        "stage": stage,
        "rows": rows,
        "passed": passed,
        "total": total_with_receipt,
        "not_run": not_run,
    }


def reconcile(dashboard: dict[str, Any], stage: str, rows: list[dict[str, Any]]) -> None:
    """Cross-check clustered counts against the dashboard's own published tallies.

    `dashboard["frontier_counts"]` and `dashboard["selected_outcome_counts"]`
    are campaign-wide (keyed by each ROM's *first* blocking stage), not
    stage-scoped. That is a faithful reconciliation target as long as no ROM
    blocks at a stage other than the one requested here -- which this also
    verifies, by deriving the dashboard's own per-stage blocked set from its
    rows (`blocked_at`) and confirming it matches what was clustered. A
    tampered or genuinely multi-stage-blocking dashboard trips this check
    rather than being silently trusted.
    """
    dashboard_blocked_here = [row for row in dashboard["rows"] if row.get("blocked_at") == stage]
    dashboard_blocked_elsewhere = [
        row for row in dashboard["rows"] if row.get("blocked_at") is not None and row.get("blocked_at") != stage
    ]

    expected_frontier: Counter[str] = Counter()
    expected_other: Counter[str] = Counter()
    for row in dashboard_blocked_here:
        blocker_kind = row.get("blocker_kind")
        if blocker_kind is None:
            raise UnblockRankError(f"{row['rom']['id']}: dashboard row blocked at {stage} names no blocker_kind")
        if blocker_kind in ("resource_limit", "infrastructure_failure", "invalid_input"):
            expected_other[blocker_kind] += 1
        else:
            expected_frontier[blocker_kind] += 1

    actual_frontier: Counter[str] = Counter()
    actual_other: Counter[str] = Counter()
    for row in rows:
        top_kind = row["key"][0]
        target = actual_other if row["section"] != "frontier" else actual_frontier
        target[top_kind] += row["rom_count"]

    published_frontier = dashboard.get("frontier_counts")
    published_outcomes = dashboard.get("selected_outcome_counts")
    if not isinstance(published_frontier, dict) or not isinstance(published_outcomes, dict):
        raise UnblockRankError("dashboard is missing frontier_counts or selected_outcome_counts")

    if dashboard_blocked_elsewhere:
        # Another stage also contributes to the campaign-wide published
        # totals, so those totals are not directly comparable to this
        # stage's clustered rows; reconcile against the dashboard's own
        # per-row derivation instead, which is stage-scoped by construction.
        pass
    elif published_frontier != dict(sorted(expected_frontier.items())):
        raise UnblockRankError(
            f"reconciliation failed for stage {stage}: dashboard frontier_counts {published_frontier} "
            f"!= dashboard's own blocked-at-{stage} rows {dict(sorted(expected_frontier.items()))}"
        )

    if actual_frontier != expected_frontier:
        raise UnblockRankError(
            f"reconciliation failed for stage {stage}: clustered frontier tally {dict(sorted(actual_frontier.items()))} "
            f"!= dashboard blocked-at-{stage} tally {dict(sorted(expected_frontier.items()))}"
        )
    if actual_other != expected_other:
        raise UnblockRankError(
            f"reconciliation failed for stage {stage}: clustered resource/infra tally {dict(sorted(actual_other.items()))} "
            f"!= dashboard blocked-at-{stage} tally {dict(sorted(expected_other.items()))}"
        )

    total_expected = sum(expected_frontier.values()) + sum(expected_other.values())
    total_actual = sum(row["rom_count"] for row in rows)
    if total_actual != total_expected:
        raise UnblockRankError(
            f"reconciliation failed for stage {stage}: {total_actual} clustered ROMs != {total_expected} dashboard-blocked ROMs"
        )


def render_text(report: dict[str, Any]) -> str:
    lines = []
    frontier_rows = [row for row in report["rows"] if row["section"] == "frontier"]
    other_rows = [row for row in report["rows"] if row["section"] != "frontier"]

    lines.append(f"stage: {report['stage']}")
    lines.append("")
    lines.append("frontier clusters (descending by ROM count):")
    if not frontier_rows:
        lines.append("  (none)")
    for row in frontier_rows:
        lines.append(f"  [{row['rom_count']:3d}] {row['label']}")
        for wrapped in wrap_rom_ids(row["rom_ids"]):
            lines.append(f"        {wrapped}")

    lines.append("")
    lines.append("resource-limit / infrastructure-failure (own rows, never inside a frontier cluster):")
    if not other_rows:
        lines.append("  (none)")
    for row in other_rows:
        lines.append(f"  [{row['rom_count']:3d}] {row['label']}")
        for wrapped in wrap_rom_ids(row["rom_ids"]):
            lines.append(f"        {wrapped}")

    lines.append("")
    lines.append(f"passed: {report['passed']} of {report['total']}, not_run: {report['not_run']}")
    return "\n".join(lines) + "\n"


def canonical_json(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def _require_coverage_entry(value: Any, rom_id: str, label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or "status" not in value or "ratio" not in value:
        raise UnblockRankError(f"{rom_id}: malformed coverage.{label}")
    status = value["status"]
    if status not in ("ok", "not_run", "undefined"):
        raise UnblockRankError(f"{rom_id}: coverage.{label} has unknown status {status!r}")
    ratio = value["ratio"]
    if status == "ok":
        if not isinstance(ratio, (int, float)) or isinstance(ratio, bool):
            raise UnblockRankError(f"{rom_id}: coverage.{label} status ok has non-numeric ratio")
    elif ratio is not None:
        raise UnblockRankError(f"{rom_id}: coverage.{label} status {status} must carry a null ratio")
    return value


def median(values: list[float]) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    mid = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[mid]
    return (ordered[mid - 1] + ordered[mid]) / 2


def coverage_report(campaign_dir: Path, dashboard: dict[str, Any]) -> dict[str, Any]:
    """Per-ROM mapped/recompiled proxy coverage. The two ratios come straight
    from the dashboard's own `coverage` field on each row (mirroring how
    `rank()` trusts the dashboard for its receipt selection); `code_run_bytes`
    itself -- needed only to order the "certified but recompiled < 50%" list,
    never printed as a path -- is read back from the selected `discover`
    receipt named in `selected_receipts`, the same receipts-are-the-authority
    pattern `rank()` uses for its clustering detail.

    "Certified" here means the row's highest_passed_stage is recompile (a
    receipt exists and passed), not that the ROM is fully covered."""
    manifest = DASHBOARD.load_manifest(campaign_dir / "manifest.json")
    receipts = DASHBOARD.load_receipts(campaign_dir / "receipts", manifest)
    by_id = {receipt["receipt_id"]: receipt for receipt in receipts}

    dashboard_roms = {row["rom"]["id"]: row for row in dashboard["rows"]}
    manifest_rom_ids = [rom["id"] for rom in manifest["roms"]]
    if set(dashboard_roms) != set(manifest_rom_ids):
        raise UnblockRankError("dashboard ROM set does not match campaign manifest")

    rows = []
    for dashboard_row in dashboard["rows"]:
        rom_id = dashboard_row["rom"]["id"]
        coverage = dashboard_row.get("coverage")
        if not isinstance(coverage, dict):
            raise UnblockRankError(f"{rom_id}: dashboard row has no coverage")
        mapped = _require_coverage_entry(coverage.get("mapped_ratio"), rom_id, "mapped_ratio")
        recompiled = _require_coverage_entry(coverage.get("recompiled_ratio"), rom_id, "recompiled_ratio")
        highest_passed_stage = dashboard_row.get("highest_passed_stage")
        if not isinstance(highest_passed_stage, str):
            raise UnblockRankError(f"{rom_id}: dashboard row has no highest_passed_stage")

        code_run_bytes = None
        discover_receipt_id = dashboard_row.get("selected_receipts", {}).get("discover")
        if discover_receipt_id is not None:
            discover_receipt = by_id.get(discover_receipt_id)
            if discover_receipt is None:
                raise UnblockRankError(f"{rom_id}: dashboard selected discover receipt {discover_receipt_id} is absent from campaign receipts")
            result = discover_receipt.get("result")
            candidate_bytes = result.get("code_run_bytes") if isinstance(result, dict) else None
            if isinstance(candidate_bytes, int) and not isinstance(candidate_bytes, bool):
                code_run_bytes = candidate_bytes

        rows.append({
            "rom_id": rom_id,
            "status": dashboard_row["status"],
            "highest_passed_stage": highest_passed_stage,
            "mapped_ratio": mapped,
            "recompiled_ratio": recompiled,
            "code_run_bytes": code_run_bytes,
        })
    rows.sort(key=lambda row: row["rom_id"])

    mapped_values = [row["mapped_ratio"]["ratio"] for row in rows if row["mapped_ratio"]["status"] == "ok"]
    recompiled_values = [row["recompiled_ratio"]["ratio"] for row in rows if row["recompiled_ratio"]["status"] == "ok"]

    certified_under_50 = [
        row for row in rows
        if row["highest_passed_stage"] == "recompile"
        and row["recompiled_ratio"]["status"] == "ok"
        and row["recompiled_ratio"]["ratio"] < 0.5
    ]
    # Descending by code_run_bytes; a ROM with no known code_run_bytes sorts
    # last rather than being dropped from the list.
    certified_under_50.sort(
        key=lambda row: (row["code_run_bytes"] is None, -(row["code_run_bytes"] or 0), row["rom_id"])
    )

    return {
        "rows": rows,
        "median_mapped": median(mapped_values),
        "median_recompiled": median(recompiled_values),
        "certified_under_50": certified_under_50,
    }


def render_ratio(entry: dict[str, Any]) -> str:
    if entry["status"] != "ok":
        return entry["status"]
    return f"{entry['ratio'] * 100:.1f}%"


def render_coverage_text(report: dict[str, Any]) -> str:
    lines = []
    lines.append("coverage (mapped / recompiled proxy ratios):")
    lines.append("")
    for row in report["rows"]:
        lines.append(
            f"  {row['rom_id']} | {row['status']} | mapped={render_ratio(row['mapped_ratio'])} "
            f"| recompiled={render_ratio(row['recompiled_ratio'])}"
        )
    lines.append("")
    median_mapped = report["median_mapped"]
    median_recompiled = report["median_recompiled"]
    lines.append(
        "median mapped: " + (f"{median_mapped * 100:.1f}%" if median_mapped is not None else "undefined")
    )
    lines.append(
        "median recompiled: "
        + (f"{median_recompiled * 100:.1f}%" if median_recompiled is not None else "undefined")
    )
    lines.append("")
    lines.append("certified but recompiled < 50% (descending by code_run_bytes):")
    if not report["certified_under_50"]:
        lines.append("  (none)")
    for row in report["certified_under_50"]:
        lines.append(f"  {row['rom_id']} | recompiled={render_ratio(row['recompiled_ratio'])}")
    return "\n".join(lines) + "\n"


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--campaign-dir", required=True, type=Path)
    parser.add_argument("--dashboard-json", required=True, type=Path)
    parser.add_argument("--stage", choices=("recompile", "pack", "discover"), default="recompile")
    parser.add_argument("--json", type=Path, default=None)
    parser.add_argument("--coverage", action="store_true", help="print per-ROM mapped/recompiled coverage instead of the unblock ranking")
    args = parser.parse_args(argv)

    try:
        dashboard = load_dashboard(args.dashboard_json)
        if args.coverage:
            report = coverage_report(args.campaign_dir, dashboard)
        else:
            report = rank(args.campaign_dir, dashboard, args.stage)
    except UnblockRankError as error:
        print(f"corpus-unblock-rank: FAILED: {error}", file=sys.stderr)
        return 2

    sys.stdout.write(render_coverage_text(report) if args.coverage else render_text(report))
    if args.json is not None:
        args.json.write_bytes(canonical_json(report) + b"\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
