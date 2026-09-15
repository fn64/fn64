#!/usr/bin/env python3
"""Run the pack+recompile stages over a corpus campaign and mint receipts.

For every manifest ROM that has a selected `passed` `discover` receipt, this
runs `gate-rom-recompile` once against a private ROM path (looked up from
`--rom-map`, never stored in the campaign) with a wall-clock timeout and RSS
sampling. The gate packs before it emits, so one run mints up to two
`fn64.corpus-stage-receipt.v1` receipts: `pack` first (predecessor: the
discover receipt), and -- only when pack passed -- `recompile` (predecessor:
the pack receipt), whose outcome comes from the `HEADLINE unsupported=N`
line as before. stdout+stderr are captured to a private artifact file under
`<campaign>/artifacts/<rom-id>/<attempt>/`; only that file's sha256 is
recorded in each receipt.

This script never reads or writes ROM bytes into the campaign and never
prints a local filesystem path to stdout or stderr.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import os
import re
import secrets
import stat
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


RECEIPT_SCHEMA = "fn64.corpus-stage-receipt.v1"
MANIFEST_SCHEMA = "fn64.corpus-campaign.v1"
PACK_STAGE = "pack"
RECOMPILE_STAGE = "recompile"
SHA256 = re.compile(r"^[0-9a-f]{64}$")
RSS_SAMPLE_INTERVAL_SECONDS = 1.0
HEADLINE_RE = re.compile(r"HEADLINE unsupported=(\d+)")
FAILED_RE = re.compile(r"FAILED:\s*(.*)")
# Strip anything that looks like an absolute filesystem path out of a
# `FAILED: <Kind> ...` detail line before it is ever written to a receipt.
PATH_RE = re.compile(r"(?:/[^\s\"']+)+")
# The first CamelCase identifier in a FAILED detail text, e.g.
# NoUniqueAdmittedTable, InvalidRangeRelations, InvalidResidentSplit,
# UnalignedField. Requires at least two capitalized humps so it does not
# match a single capitalized word.
CAMEL_CASE_RE = re.compile(r"\b[A-Z][a-z]+(?:[A-Z][a-z0-9]+)+\b")


class SweepError(Exception):
    """A loud, actionable failure. Never a silent skip."""


def canonical_json(value: Any) -> bytes:
    try:
        return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
    except (TypeError, ValueError) as error:
        raise SweepError("value is not canonical JSON") from error


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def require_sha(value: Any, label: str) -> str:
    if not isinstance(value, str) or not SHA256.fullmatch(value):
        raise SweepError(f"{label} is not a lowercase SHA-256")
    return value


def read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise SweepError(f"cannot read a required input file: {error}") from error
    if not isinstance(value, dict):
        raise SweepError("a required input file is not a JSON object")
    return value


def strip_paths(text: str) -> str:
    return PATH_RE.sub("<path>", text)


def load_manifest(campaign_dir: Path) -> dict[str, Any]:
    manifest = read_json(campaign_dir / "manifest.json")
    if manifest.get("schema") != MANIFEST_SCHEMA:
        raise SweepError(f"manifest is not {MANIFEST_SCHEMA}")
    roms = manifest.get("roms")
    if not isinstance(roms, list) or not roms:
        raise SweepError("manifest roms is empty")
    for rom in roms:
        if not isinstance(rom, dict) or not isinstance(rom.get("id"), str):
            raise SweepError("manifest ROM has no string id")
        require_sha(rom.get("normalized_sha256"), f"manifest ROM {rom['id']}")
    return manifest


def load_rom_map(path: Path) -> dict[str, str]:
    """A private sha256 -> absolute ROM path table. Never part of the manifest."""
    text = path.read_text()
    entries: dict[str, str] = {}
    stripped = text.strip()
    if stripped.startswith("["):
        records = json.loads(stripped)
    elif stripped.startswith("{") and "\n" not in stripped.strip():
        # A single JSON object mapping digest -> path.
        records = json.loads(stripped)
        if isinstance(records, dict):
            for key, value in records.items():
                require_sha(key, "rom-map key")
                if not isinstance(value, str):
                    raise SweepError(f"rom-map entry for {key} is not a string path")
                entries[key] = value
            return entries
        raise SweepError("rom-map JSON is not an object or array")
    else:
        records = [json.loads(line) for line in text.splitlines() if line.strip()]
    if isinstance(records, list):
        for record in records:
            if not isinstance(record, dict):
                raise SweepError("rom-map record is not an object")
            digest = require_sha(record.get("normalized_sha256"), "rom-map record")
            path_text = record.get("path")
            if not isinstance(path_text, str) or not path_text:
                raise SweepError(f"rom-map record for {digest} has no path")
            entries[digest] = path_text
        return entries
    raise SweepError("rom-map is not a JSONL/JSON list of records")


def load_discover_receipts(campaign_dir: Path, manifest: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """Selected passed `discover` receipt per ROM id: newest `finished_at` wins."""
    receipt_root = campaign_dir / "receipts"
    if not receipt_root.is_dir():
        raise SweepError("campaign has no receipts directory")
    manifest_ids = {rom["id"] for rom in manifest["roms"]}
    by_rom: dict[str, list[dict[str, Any]]] = {}
    for path in sorted(receipt_root.rglob("*.json")):
        value = read_json(path)
        if value.get("schema") != RECEIPT_SCHEMA or value.get("stage") != "discover":
            continue
        rom = value.get("rom")
        if not isinstance(rom, dict) or rom.get("id") not in manifest_ids:
            continue
        outcome = value.get("outcome")
        if not isinstance(outcome, dict) or outcome.get("kind") != "passed":
            continue
        if not isinstance(value.get("finished_at"), str) or not isinstance(value.get("receipt_id"), str):
            continue
        by_rom.setdefault(rom["id"], []).append(value)
    selected: dict[str, dict[str, Any]] = {}
    for rom_id, receipts in by_rom.items():
        newest = max(item["finished_at"] for item in receipts)
        winners = [item for item in receipts if item["finished_at"] == newest]
        if len(winners) != 1:
            raise SweepError(f"{rom_id}: ambiguous newest passed discover receipt")
        selected[rom_id] = winners[0]
    return selected


def scan_existing_receipts_for_candidate(
    campaign_dir: Path, git_commit: str, binary_sha256: str
) -> tuple[dict[str, dict[str, Any]], set[str]]:
    """Return (rom_id -> selected pack receipt, {rom_id with any recompile receipt}).

    Only receipts bound to this exact candidate identity are considered.
    """
    receipt_root = campaign_dir / "receipts"
    if not receipt_root.is_dir():
        return {}, set()
    pack_by_rom: dict[str, list[dict[str, Any]]] = {}
    recompiled: set[str] = set()
    for path in sorted(receipt_root.rglob("*.json")):
        value = read_json(path)
        if value.get("schema") != RECEIPT_SCHEMA:
            continue
        candidate = value.get("candidate")
        if not isinstance(candidate, dict):
            continue
        if candidate.get("git_commit") != git_commit or candidate.get("binary_sha256") != binary_sha256:
            continue
        rom = value.get("rom")
        if not isinstance(rom, dict) or not isinstance(rom.get("id"), str):
            continue
        rom_id = rom["id"]
        stage = value.get("stage")
        if stage == PACK_STAGE:
            pack_by_rom.setdefault(rom_id, []).append(value)
        elif stage == RECOMPILE_STAGE:
            recompiled.add(rom_id)
    selected_pack: dict[str, dict[str, Any]] = {}
    for rom_id, receipts in pack_by_rom.items():
        newest = max(item.get("finished_at", "") for item in receipts)
        winners = [item for item in receipts if item.get("finished_at", "") == newest]
        selected_pack[rom_id] = winners[0]
    return selected_pack, recompiled


def resumable_rom_ids(
    selected_pack: dict[str, dict[str, Any]], recompiled: set[str]
) -> set[str]:
    """ROM ids that already hold a pack receipt AND (a recompile receipt OR a
    non-passed pack receipt) for this candidate -- nothing further to attempt."""
    done = set()
    for rom_id, pack_receipt in selected_pack.items():
        outcome = pack_receipt.get("outcome")
        pack_passed = isinstance(outcome, dict) and outcome.get("kind") == "passed"
        if rom_id in recompiled or not pack_passed:
            done.add(rom_id)
    return done


def load_candidate(path: Path) -> dict[str, Any]:
    value = read_json(path)
    if not isinstance(value.get("git_commit"), str) or not value["git_commit"]:
        raise SweepError("candidate.git_commit is required")
    has_worktree_sha = isinstance(value.get("worktree_sha256"), str) and value["worktree_sha256"]
    has_dirty_flag = isinstance(value.get("dirty"), bool)
    if not has_worktree_sha and not has_dirty_flag:
        raise SweepError("candidate must set worktree_sha256 or dirty")
    if not isinstance(value.get("binary_sha256"), str) or not value["binary_sha256"]:
        raise SweepError("candidate.binary_sha256 is required")
    if not isinstance(value.get("toolchain"), str) or not value["toolchain"]:
        raise SweepError("candidate.toolchain is required")
    return value


def load_rom_ids_filter(path: Path) -> set[str]:
    return {line.strip() for line in path.read_text().splitlines() if line.strip()}


def sample_rss_bytes(pid: int) -> int | None:
    """POSIX `ps` RSS sample in bytes, mirroring rom-frontier.py's method."""
    try:
        sampled = subprocess.run(
            ["ps", "-o", "rss=", "-p", str(pid)],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=1,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if sampled.returncode != 0:
        return None
    try:
        kibibytes = int(sampled.stdout.strip())
    except ValueError:
        return None
    return kibibytes * 1024


def run_with_timeout(
    argv: list[str], timeout_seconds: int, max_rss_bytes: int | None, env: dict[str, str]
) -> dict[str, Any]:
    """Run argv, sampling RSS like rom-frontier.py. Never raises on ROM failure."""
    started = time.monotonic()
    try:
        process = subprocess.Popen(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )
    except OSError as error:
        return {
            "timed_out": False,
            "rss_exceeded": False,
            "exit_code": None,
            "signal": None,
            "stdout": b"",
            "stderr": str(error).encode(),
            "wall_seconds": 0.0,
            "peak_rss_bytes": None,
        }

    peak_rss_bytes = sample_rss_bytes(process.pid)
    timed_out = False
    rss_exceeded = False
    while True:
        if max_rss_bytes is not None and peak_rss_bytes is not None and peak_rss_bytes > max_rss_bytes:
            rss_exceeded = True
            process.kill()
            process.communicate()
            break
        remaining = timeout_seconds - (time.monotonic() - started)
        if remaining <= 0:
            timed_out = True
            process.kill()
            process.communicate()
            break
        try:
            stdout, stderr = process.communicate(timeout=min(RSS_SAMPLE_INTERVAL_SECONDS, remaining))
            break
        except subprocess.TimeoutExpired:
            rss_bytes = sample_rss_bytes(process.pid)
            if rss_bytes is not None:
                peak_rss_bytes = max(peak_rss_bytes or 0, rss_bytes)

    wall_seconds = round(time.monotonic() - started, 6)
    if timed_out or rss_exceeded:
        return {
            "timed_out": timed_out,
            "rss_exceeded": rss_exceeded,
            "exit_code": None,
            "signal": None,
            "stdout": b"",
            "stderr": b"",
            "wall_seconds": wall_seconds,
            "peak_rss_bytes": peak_rss_bytes,
        }
    return {
        "timed_out": False,
        "rss_exceeded": False,
        "exit_code": process.returncode,
        "signal": None,
        "stdout": stdout,
        "stderr": stderr,
        "wall_seconds": wall_seconds,
        "peak_rss_bytes": peak_rss_bytes,
    }


def parse_headline(stdout_text: str) -> tuple[str, Any]:
    """Return (kind, payload) from a gate-rom-recompile transcript.

    kind is one of: "unsupported", "failed", "none".
    """
    failed_line = None
    for line in stdout_text.splitlines():
        headline = HEADLINE_RE.search(line)
        if headline:
            return "unsupported", int(headline.group(1))
        match = FAILED_RE.search(line)
        if match:
            failed_line = match.group(1)
    if failed_line is not None:
        return "failed", failed_line
    return "none", None


def failed_kind_phase_and_detail(failed_line: str) -> tuple[str, str | None, str]:
    """Derive (kind, phase, detail) from a `FAILED: ...` detail line.

    The frontier kind is the first CamelCase identifier anywhere in the text
    (e.g. NoUniqueAdmittedTable, InvalidResidentSplit); if none is found,
    fall back to the first token, as before. The phase word -- the text
    between `FAILED:` and the first colon -- is recorded separately
    whenever a colon is present; it is None for the plain fallback shape
    (no colon at all, e.g. a bare `FAILED: SomeKind reason text`).
    """
    text = failed_line.strip()
    phase = None
    colon_index = text.find(":")
    if colon_index != -1:
        candidate_phase = text[:colon_index].strip()
        if candidate_phase:
            phase = candidate_phase
    camel_match = CAMEL_CASE_RE.search(text)
    if camel_match:
        return camel_match.group(0), phase, strip_paths(text)
    tokens = text.split(None, 1)
    kind = tokens[0].rstrip(":") if tokens else "Unknown"
    detail = tokens[1] if len(tokens) > 1 else ""
    return kind, None, strip_paths(detail)


def reason_tag(reason: Any) -> str | None:
    if isinstance(reason, str):
        return reason
    if isinstance(reason, dict):
        tag = reason.get("kind") or next(iter(reason), None)
        if isinstance(tag, str):
            return tag
    return None


def hex_va(value: Any) -> str | None:
    if isinstance(value, bool) or not isinstance(value, int):
        return None
    return hex(value)


def run_diagnose_cold_unsupported(
    binary: Path, rom_path: Path, timeout_seconds: int, env: dict[str, str]
) -> tuple[list[str], bool, bytes | None, list[dict[str, Any]]]:
    """Run diagnose-cold-unsupported and return:

    (sorted unique DestinationReason strings, diagnostic_failed,
     raw JSON line bytes or None, per-destination address-only summaries).

    The per-destination summaries are {destination_va (0x hex string),
    reason, incoming_kinds (sorted unique kinds)} -- addresses and
    classifications only, never paths or bytes.
    """
    result = run_with_timeout(
        [str(binary), "diagnose-cold-unsupported", str(rom_path)], timeout_seconds, None, env
    )
    if result["timed_out"] or result["rss_exceeded"] or result["exit_code"] != 0:
        return [], True, None, []
    reasons: set[str] = set()
    unsupported: list[dict[str, Any]] = []
    json_line: bytes | None = None
    try:
        for line in result["stdout"].decode("utf-8", "replace").splitlines():
            if not line.startswith("{"):
                continue
            record = json.loads(line)
            destinations = record.get("unsupported_destinations", [])
            for destination in destinations:
                reason_value = destination.get("reason")
                tag = reason_tag(reason_value)
                if tag is not None:
                    reasons.add(tag)
                incoming_kinds: set[str] = set()
                for edge in destination.get("incoming", []) or []:
                    if isinstance(edge, dict):
                        edge_kind = edge.get("kind")
                        if isinstance(edge_kind, str):
                            incoming_kinds.add(edge_kind)
                unsupported.append({
                    "destination_va": hex_va(destination.get("destination_va")),
                    "reason": tag,
                    "incoming_kinds": sorted(incoming_kinds),
                })
            # Retain the JSON line verbatim as the private artifact; take the
            # first record containing unsupported_destinations.
            if json_line is None and destinations:
                json_line = line.encode("utf-8") if isinstance(line, str) else line
    except (json.JSONDecodeError, AttributeError, TypeError):
        return [], True, None, []
    return sorted(reasons), False, json_line, unsupported


def write_new(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.parent / f".{path.name}.tmp-{os.getpid()}-{secrets.token_hex(8)}"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, stat.S_IRUSR | stat.S_IWUSR)
    try:
        os.write(descriptor, content)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    try:
        os.link(temporary, path)
    except FileExistsError as error:
        raise SweepError("refusing to overwrite an existing receipt/artifact") from error
    finally:
        os.unlink(temporary)


def utc_now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def build_receipt(
    *,
    stage: str,
    campaign_id: str,
    attempt_id: str,
    rom_id: str,
    rom_sha256: str,
    predecessor_receipt_id: str,
    candidate: dict[str, Any],
    policy: dict[str, Any],
    outcome_kind: str,
    frontier: dict[str, Any] | None,
    result: dict[str, Any],
    artifact_sha256: str,
    started_at: str,
    finished_at: str,
    extra_artifacts: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    artifacts = [{"kind": "stdout", "sha256": artifact_sha256, "visibility": "private"}]
    if extra_artifacts:
        artifacts.extend(extra_artifacts)
    body = {
        "schema": RECEIPT_SCHEMA,
        "campaign_id": campaign_id,
        "attempt_id": attempt_id,
        "stage": stage,
        "rom": {"id": rom_id, "normalized_sha256": rom_sha256},
        "candidate": candidate,
        "predecessors": [predecessor_receipt_id],
        "policy": policy,
        "outcome": {"kind": outcome_kind, "frontier": frontier},
        "result": result,
        "artifacts": artifacts,
        "started_at": started_at,
        "finished_at": finished_at,
    }
    receipt_id = stage + "-" + sha256_bytes(canonical_json(body))
    body = {"receipt_id": receipt_id, **body}
    return body


def write_receipt(campaign_dir: Path, receipt: dict[str, Any]) -> None:
    receipts_dir = campaign_dir / "receipts"
    receipts_dir.mkdir(parents=True, exist_ok=True)
    write_new(receipts_dir / f"{receipt['receipt_id']}.json", canonical_json(receipt) + b"\n")


def pack_result_fields(report_payload: dict[str, Any]) -> dict[str, Any]:
    fields = {}
    # `supported_banks` (B8/K20) is retained beside `banks` so a campaign
    # receipt records how much of a ROM's pack rests on a Supported placement
    # rather than a proof. It is absent from reports written before K20, and
    # the membership test below keeps those parsing unchanged.
    for key in (
        "normalized_rom_sha256",
        "internal_name",
        "banks",
        "supported_banks",
        "pack_blocks",
        "pack_words",
    ):
        if key in report_payload:
            fields[key] = report_payload[key]
    return fields


def sweep_one(
    *,
    binary: Path,
    rom_path: Path,
    rom_id: str,
    rom_sha256: str,
    discover_receipt: dict[str, Any],
    candidate: dict[str, Any],
    candidate_env: dict[str, str],
    timeout_seconds: int,
    max_rss_bytes: int | None,
    campaign_dir: Path,
    campaign_id: str,
    attempt_id: str,
) -> list[dict[str, Any]]:
    """Run one gate-rom-recompile and mint up to two receipts: pack, then
    recompile (only when pack passed). Returns the minted receipts in order."""
    artifact_dir = campaign_dir / "artifacts" / rom_id / attempt_id
    artifact_dir.mkdir(parents=True, exist_ok=True)
    report_path = artifact_dir / "recompile-report.json"
    if report_path.exists():
        report_path.unlink()

    env = dict(candidate_env)
    env["FN64_DISCOVER_ROM"] = str(rom_path)
    env["FN64_RECOMPILE_REPORT"] = str(report_path)

    started_at = utc_now_iso()
    result = run_with_timeout(
        [str(binary), "gate-rom-recompile"], timeout_seconds, max_rss_bytes, env
    )
    finished_at = utc_now_iso()

    combined = result["stdout"] + b"\n--- stderr ---\n" + result["stderr"]
    artifact_text_path = artifact_dir / "transcript.log"
    if artifact_text_path.exists():
        artifact_text_path.unlink()
    write_new(artifact_text_path, combined)
    artifact_sha256 = sha256_file(artifact_text_path)

    policy = {
        "wall_time_ms": int(result["wall_seconds"] * 1000),
        "peak_rss_bytes": result["peak_rss_bytes"],
        "timeout_seconds": timeout_seconds,
    }

    report_payload: dict[str, Any] = {}
    if report_path.exists():
        try:
            report_payload = json.loads(report_path.read_text())
        except (OSError, json.JSONDecodeError):
            report_payload = {}

    pack_words = report_payload.get("pack_words")
    has_report = bool(report_payload)
    pack_ok = has_report and isinstance(pack_words, int) and pack_words > 0

    # --- pack outcome -----------------------------------------------------
    if result["timed_out"]:
        pack_outcome_kind = "resource_limit"
        pack_frontier = None
        pack_payload = {"resource_limit": {"which": "wall_time", "timeout_seconds": timeout_seconds}}
        print(f"corpus-recompile-sweep: {rom_id} pack resource_limit(wall_time)", file=sys.stderr)
    elif result["rss_exceeded"]:
        pack_outcome_kind = "resource_limit"
        pack_frontier = None
        pack_payload = {"resource_limit": {"which": "rss", "max_rss_bytes": max_rss_bytes}}
        print(f"corpus-recompile-sweep: {rom_id} pack resource_limit(rss)", file=sys.stderr)
    else:
        # review-gate.sh greps `2>&1`, so the FAILED line (eprintln!) and the
        # HEADLINE line (println!) are both in scope here.
        combined_text = (result["stdout"] + result["stderr"]).decode("utf-8", "replace")
        kind, payload_value = parse_headline(combined_text)
        if pack_ok:
            pack_outcome_kind = "passed"
            pack_frontier = None
            pack_payload = {}
            print(f"corpus-recompile-sweep: {rom_id} pack passed", file=sys.stderr)
        elif kind == "failed":
            pack_outcome_kind = "frontier"
            failed_kind, failed_phase, detail = failed_kind_phase_and_detail(payload_value)
            pack_frontier = {"kind": failed_kind, "detail": detail}
            if failed_phase is not None:
                pack_frontier["phase"] = failed_phase
            pack_payload = {}
            print(f"corpus-recompile-sweep: {rom_id} pack frontier({failed_kind})", file=sys.stderr)
        elif result["exit_code"] not in (0,):
            pack_outcome_kind = "infrastructure_failure"
            pack_frontier = None
            pack_payload = {"exit_code": result["exit_code"]}
            print(f"corpus-recompile-sweep: {rom_id} pack infrastructure_failure(exit)", file=sys.stderr)
        else:
            pack_outcome_kind = "infrastructure_failure"
            pack_frontier = None
            pack_payload = {}
            print(f"corpus-recompile-sweep: {rom_id} pack infrastructure_failure(no_report)", file=sys.stderr)

    pack_result = dict(pack_result_fields(report_payload))
    pack_result.update(pack_payload)

    pack_receipt = build_receipt(
        stage=PACK_STAGE,
        campaign_id=campaign_id,
        attempt_id=attempt_id,
        rom_id=rom_id,
        rom_sha256=rom_sha256,
        predecessor_receipt_id=discover_receipt["receipt_id"],
        candidate=candidate,
        policy=policy,
        outcome_kind=pack_outcome_kind,
        frontier=pack_frontier,
        result=pack_result,
        artifact_sha256=artifact_sha256,
        started_at=started_at,
        finished_at=finished_at,
    )
    write_receipt(campaign_dir, pack_receipt)
    receipts = [pack_receipt]

    if pack_outcome_kind != "passed":
        return receipts

    # --- recompile outcome (only reached when pack passed) ----------------
    combined_text = (result["stdout"] + result["stderr"]).decode("utf-8", "replace")
    kind, payload_value = parse_headline(combined_text)
    recompile_extra_artifacts: list[dict[str, Any]] = []
    if kind == "unsupported" and payload_value == 0:
        outcome_kind = "passed"
        frontier = None
        payload = {"headline": "unsupported=0"}
        print(f"corpus-recompile-sweep: {rom_id} recompile passed", file=sys.stderr)
    elif kind == "unsupported":
        outcome_kind = "frontier"
        reasons, diagnostic_failed, diagnostic_json_line, unsupported = run_diagnose_cold_unsupported(
            binary, rom_path, timeout_seconds, candidate_env
        )
        frontier = {
            "kind": "unsupported_destinations",
            "count": payload_value,
            "reasons": reasons,
        }
        if diagnostic_failed:
            frontier["diagnostic_failed"] = True
        payload = {"headline": f"unsupported={payload_value}"}
        if unsupported:
            payload["unsupported"] = unsupported
        if diagnostic_json_line is not None:
            cold_unsupported_path = artifact_dir / "cold-unsupported.json"
            if cold_unsupported_path.exists():
                cold_unsupported_path.unlink()
            write_new(cold_unsupported_path, diagnostic_json_line.rstrip(b"\n") + b"\n")
            cold_unsupported_sha256 = sha256_file(cold_unsupported_path)
            recompile_extra_artifacts.append({
                "kind": "cold_unsupported",
                "sha256": cold_unsupported_sha256,
                "visibility": "private",
            })
        print(
            f"corpus-recompile-sweep: {rom_id} recompile frontier(unsupported_destinations={payload_value})",
            file=sys.stderr,
        )
    elif kind == "failed":
        # Pack passed (a report with pack_words>0 exists) but the run still
        # ended with a FAILED line after packing -- a recompile-stage
        # frontier, distinct from a pack frontier.
        outcome_kind = "frontier"
        failed_kind, failed_phase, detail = failed_kind_phase_and_detail(payload_value)
        frontier = {"kind": failed_kind, "detail": detail}
        if failed_phase is not None:
            frontier["phase"] = failed_phase
        payload = {"headline": f"FAILED: {failed_kind}"}
        print(f"corpus-recompile-sweep: {rom_id} recompile frontier({failed_kind})", file=sys.stderr)
    elif result["exit_code"] not in (0,):
        outcome_kind = "infrastructure_failure"
        frontier = None
        payload = {"headline": None, "exit_code": result["exit_code"]}
        print(f"corpus-recompile-sweep: {rom_id} recompile infrastructure_failure(exit)", file=sys.stderr)
    else:
        outcome_kind = "infrastructure_failure"
        frontier = None
        payload = {"headline": None}
        print(f"corpus-recompile-sweep: {rom_id} recompile infrastructure_failure(no_headline)", file=sys.stderr)

    receipt_result = dict(report_payload)
    receipt_result.update(payload)

    recompile_receipt = build_receipt(
        stage=RECOMPILE_STAGE,
        campaign_id=campaign_id,
        attempt_id=attempt_id,
        rom_id=rom_id,
        rom_sha256=rom_sha256,
        predecessor_receipt_id=pack_receipt["receipt_id"],
        candidate=candidate,
        policy=policy,
        outcome_kind=outcome_kind,
        frontier=frontier,
        result=receipt_result,
        artifact_sha256=artifact_sha256,
        started_at=started_at,
        finished_at=finished_at,
        extra_artifacts=recompile_extra_artifacts,
    )
    write_receipt(campaign_dir, recompile_receipt)
    receipts.append(recompile_receipt)
    return receipts


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--campaign-dir", required=True, type=Path)
    result.add_argument("--binary", required=True, type=Path)
    result.add_argument("--rom-map", required=True, type=Path, help="private sha256 -> ROM path table")
    result.add_argument("--candidate", required=True, type=Path)
    result.add_argument("--jobs", type=int, default=2)
    result.add_argument("--timeout-seconds", type=int, default=1200)
    result.add_argument("--max-rss-bytes", type=int)
    result.add_argument("--limit", type=int)
    result.add_argument("--rom-ids", type=Path)
    result.add_argument("--resume", action="store_true")
    result.add_argument("--attempt-id", default=None)
    return result


def main(argv: list[str]) -> int:
    args = parser().parse_args(argv)
    try:
        if args.jobs <= 0:
            raise SweepError("jobs must be positive")
        if not args.binary.is_file() or not os.access(args.binary, os.X_OK):
            raise SweepError("binary is not an executable file")

        manifest = load_manifest(args.campaign_dir)
        campaign_id = manifest["campaign_id"]
        rom_map = load_rom_map(args.rom_map)
        candidate = load_candidate(args.candidate)
        discover_receipts = load_discover_receipts(args.campaign_dir, manifest)

        candidate_block = {
            "git_commit": candidate["git_commit"],
            "binary_sha256": candidate["binary_sha256"],
            "toolchain": candidate["toolchain"],
        }
        if "worktree_sha256" in candidate and candidate["worktree_sha256"]:
            candidate_block["worktree_sha256"] = candidate["worktree_sha256"]
        if "dirty" in candidate:
            candidate_block["dirty"] = candidate["dirty"]
        if "features" in candidate:
            candidate_block["features"] = candidate["features"]

        already_done: set[str] = set()
        if args.resume:
            selected_pack, recompiled = scan_existing_receipts_for_candidate(
                args.campaign_dir, candidate["git_commit"], candidate["binary_sha256"]
            )
            already_done = resumable_rom_ids(selected_pack, recompiled)

        rom_ids_filter = load_rom_ids_filter(args.rom_ids) if args.rom_ids else None

        candidate_env = dict(os.environ)

        eligible = []
        for rom in manifest["roms"]:
            rom_id = rom["id"]
            if rom_ids_filter is not None and rom_id not in rom_ids_filter:
                continue
            if rom_id not in discover_receipts:
                continue
            if args.resume and rom_id in already_done:
                continue
            eligible.append(rom)
        if args.limit is not None:
            eligible = eligible[: args.limit]

        attempt_id = args.attempt_id or f"recompile-{secrets.token_hex(4)}"

        outcome_counts: dict[str, dict[str, int]] = {}
        skipped_no_path = 0
        skipped_resumed = len(already_done) if args.resume else 0

        runnable: list[tuple[str, str, Path]] = []
        for rom in eligible:
            rom_id = rom["id"]
            rom_sha256 = rom["normalized_sha256"]
            rom_path_text = rom_map.get(rom_sha256)
            if not rom_path_text:
                skipped_no_path += 1
                print(f"corpus-recompile-sweep: {rom_id} skip(no_rom_path)", file=sys.stderr)
                continue
            rom_path = Path(rom_path_text)
            if not rom_path.is_file():
                skipped_no_path += 1
                print(f"corpus-recompile-sweep: {rom_id} skip(rom_path_missing)", file=sys.stderr)
                continue
            runnable.append((rom_id, rom_sha256, rom_path))

        # Each ROM's recompile attempt is independent; run them concurrently
        # the way rom-frontier.py runs discovery, bounded by --jobs.
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
            pending = {
                pool.submit(
                    sweep_one,
                    binary=args.binary,
                    rom_path=rom_path,
                    rom_id=rom_id,
                    rom_sha256=rom_sha256,
                    discover_receipt=discover_receipts[rom_id],
                    candidate=candidate_block,
                    candidate_env=candidate_env,
                    timeout_seconds=args.timeout_seconds,
                    max_rss_bytes=args.max_rss_bytes,
                    campaign_dir=args.campaign_dir,
                    campaign_id=campaign_id,
                    attempt_id=attempt_id,
                ): rom_id
                for rom_id, rom_sha256, rom_path in runnable
            }
            for future in concurrent.futures.as_completed(pending):
                receipts = future.result()
                for receipt in receipts:
                    stage_counts = outcome_counts.setdefault(receipt["stage"], {})
                    kind = receipt["outcome"]["kind"]
                    stage_counts[kind] = stage_counts.get(kind, 0) + 1

        summary = {
            "attempted": len(runnable),
            "skipped_no_rom_path": skipped_no_path,
            "skipped_resumed": skipped_resumed,
            "outcome_counts": outcome_counts,
        }
        print(json.dumps(summary, sort_keys=True))
    except SweepError as error:
        print(f"corpus-recompile-sweep: FAILED: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
