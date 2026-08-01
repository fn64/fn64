#!/usr/bin/env python3
"""Join a ROM catalog to discovery outcomes and rank the open frontier.

Reads `fn64.rom-catalog.v1` records and the `--summary` JSON that
`fn64-discover` prints per ROM, then answers the question the corpus exists to
answer: which ROMs does discovery fail on, and which of those failures are
closest to being fixable.

Aggregate tables go to stdout; per-ROM detail goes to `--output` only. Nothing
here reads ROM bytes -- it consumes the catalog's measurements and the
discovery receipts, both of which are already path-free.
"""

from __future__ import annotations

import argparse
import collections
import concurrent.futures
import hashlib
import json
import os
import secrets
import stat
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Iterable


FRONTIER_SCHEMA = "fn64.rom-frontier.v2"
FAILURE_SCHEMA = "fn64.rom-frontier-failure.v1"
CATALOG_SCHEMA = "fn64.rom-catalog.v1"
RSS_SAMPLE_INTERVAL_SECONDS = 1.0

# A boot bank whose call targets vastly outnumber its resident returns is a
# loader stub: its code is streamed in, so no boot-bank-only strategy can
# succeed. Below the floor the code is resident and discovery has no
# streaming excuse for failing.
RESIDENT_STUB_RATIO_CEILING = 2.0

# Above this, the boot copy is compressed and static decode is meaningless
# until something decompresses it -- a different problem class from missing
# geometry, ranked separately rather than mixed in.
COMPRESSED_ENTROPY_FLOOR = 7.0


class FrontierError(Exception):
    """A loud, actionable failure. Never a silent skip."""


def canonical_sorted(value: Any) -> bytes:
    try:
        return json.dumps(
            value,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
            allow_nan=False,
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise FrontierError("value cannot be encoded as canonical JSON") from error


def validate_output_destination(path_text: str) -> Path:
    path = Path(path_text)
    if not path.is_absolute() or ".." in path.parts or path.name in ("", ".", ".."):
        raise FrontierError("output must be an absolute new file path without '..'")
    parent = path.parent
    try:
        if parent.resolve(strict=True) != parent:
            raise FrontierError("output parent must be canonical and contain no symlinks")
        parent_info = parent.lstat()
    except OSError as error:
        raise FrontierError("cannot inspect output parent") from error
    if not stat.S_ISDIR(parent_info.st_mode):
        raise FrontierError("output parent must be an existing directory")
    try:
        path.lstat()
    except FileNotFoundError:
        return path
    except OSError as error:
        raise FrontierError("cannot inspect output destination") from error
    raise FrontierError("refusing to overwrite existing output destination")


def publish_records(path: Path, records: Iterable[dict[str, Any]]) -> None:
    validate_output_destination(str(path))
    payload = b"".join(canonical_sorted(record) + b"\n" for record in records)
    temporary = path.parent / f".{path.name}.tmp-{os.getpid()}-{secrets.token_hex(8)}"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(temporary, flags, 0o644)
    try:
        os.write(descriptor, payload)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    try:
        os.link(temporary, path)
    except FileExistsError as error:
        raise FrontierError("refusing to overwrite existing output destination") from error
    finally:
        os.unlink(temporary)


def load_catalog(path: Path) -> list[dict[str, Any]]:
    records = []
    for number, line in enumerate(path.read_text().splitlines(), start=1):
        if not line.strip():
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as error:
            raise FrontierError(f"catalog line {number} is not valid JSON") from error
        if record.get("schema") != CATALOG_SCHEMA:
            raise FrontierError(f"catalog line {number} is not {CATALOG_SCHEMA}")
        records.append(record)
    if not records:
        raise FrontierError("catalog is empty")
    return records


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def sample_rss_bytes(pid: int) -> int | None:
    """Return the child's currently observed RSS, or None when unavailable.

    POSIX `ps` reports RSS in KiB on both macOS and Linux. Sampling is used
    instead of process-global `getrusage(RUSAGE_CHILDREN)`, whose totals cannot
    be attributed correctly while corpus children run concurrently.
    """
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


def encoded_summary_from_receipt(line: str) -> bytes:
    """Recover the exact nested summary encoding covered by Rust's hash."""
    marker = '"summary":'
    marker_start = line.find(marker)
    if marker_start < 0:
        raise FrontierError("summary receipt has no summary member")
    start = marker_start + len(marker)
    if start >= len(line) or line[start] != "{":
        raise FrontierError("summary receipt summary is not an object")
    depth = 0
    quoted = False
    escaped = False
    for index in range(start, len(line)):
        character = line[index]
        if quoted:
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == '"':
                quoted = False
            continue
        if character == '"':
            quoted = True
        elif character == "{":
            depth += 1
        elif character == "}":
            depth -= 1
            if depth == 0:
                return line[start : index + 1].encode("utf-8")
    raise FrontierError("summary receipt summary is truncated")


def run_discovery(binary: Path, rom_path: Path, timeout_seconds: int) -> dict[str, Any]:
    """Run owner proof and return its summary plus attributable cost."""
    started = time.monotonic()
    try:
        process = subprocess.Popen(
            [str(binary), str(rom_path), "--summary", "--prove-owners"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as error:
        raise FrontierError(f"{rom_path.name} discovery could not start") from error

    peak_rss_bytes = sample_rss_bytes(process.pid)
    while True:
        remaining = timeout_seconds - (time.monotonic() - started)
        if remaining <= 0:
            process.kill()
            process.communicate()
            raise FrontierError(f"{rom_path.name} exceeded {timeout_seconds}s")
        try:
            stdout, _stderr = process.communicate(
                timeout=min(RSS_SAMPLE_INTERVAL_SECONDS, remaining)
            )
            break
        except subprocess.TimeoutExpired:
            rss_bytes = sample_rss_bytes(process.pid)
            if rss_bytes is not None:
                peak_rss_bytes = max(peak_rss_bytes or 0, rss_bytes)

    wall_seconds = round(time.monotonic() - started, 6)
    if process.returncode != 0:
        raise FrontierError(f"{rom_path.name} discovery exited {process.returncode}")
    for line in stdout.decode("utf-8", "replace").splitlines():
        if line.startswith("{"):
            try:
                receipt = json.loads(line)
                summary = receipt["summary"]
                claimed_hash = receipt["receipt_sha256"]
                encoded_summary = encoded_summary_from_receipt(line)
            except (json.JSONDecodeError, KeyError, TypeError, ValueError, FrontierError) as error:
                raise FrontierError(f"{rom_path.name} produced an invalid summary record") from error
            if not isinstance(summary, dict) or summary.get("schema_version") != 2:
                raise FrontierError(f"{rom_path.name} owner-proof summary is not schema v2")
            if not isinstance(claimed_hash, str) or claimed_hash != hashlib.sha256(
                encoded_summary
            ).hexdigest():
                raise FrontierError(f"{rom_path.name} summary receipt hash does not match")
            if "owner_proof" not in summary:
                raise FrontierError(f"{rom_path.name} produced no owner-proof diagnostics")
            return {
                "summary": summary,
                "wall_seconds": wall_seconds,
                "peak_rss_bytes": peak_rss_bytes,
            }
    raise FrontierError(f"{rom_path.name} produced no summary record")


"""Geometry-stage strategies. Everything else maps the boot copy only, so a
failure there is not a geometry failure -- it never reached the stage."""
GEOMETRY_STRATEGIES = ("recovered_overlays", "recovered_vrom")


def geometry_failure(outcomes: list[dict[str, Any]]) -> str:
    """Why load-image recovery did not produce a multi-bank mapping.

    Load geometry is the bottleneck: ROMs that recover it harvest roughly ten
    times what boot-bank-only ROMs do. Naming the specific unmet condition is
    what makes 276 failures actionable rather than uniform.
    """
    by_strategy = {outcome["strategy"]: outcome for outcome in outcomes}
    geometry = [by_strategy[name] for name in GEOMETRY_STRATEGIES if name in by_strategy]
    if not geometry:
        return "no_geometry_strategy_ran"

    if any(outcome["proven_mappings"] > 1 for outcome in geometry):
        return "recovered"

    # A resource ceiling is a frontier, not proven absence, so it outranks the
    # emptier verdicts below: the search was truncated, not exhausted.
    if any(outcome["decoded_file_limit_hits"] for outcome in geometry):
        return "decode_limit_hit"
    if any(outcome["physical_wrapper_candidate_limit_hit"] for outcome in geometry):
        return "wrapper_limit_hit"
    if any(outcome["request_dma_input_limit_hit"] for outcome in geometry):
        return "request_dma_limit_hit"

    if any(outcome["request_dma_incomplete"] for outcome in geometry):
        return "request_dma_incomplete"
    if any(outcome["request_dma_open_rows"] for outcome in geometry):
        return "request_dma_open_rows"

    admitted = sum(outcome["admitted_tables"] for outcome in geometry)
    candidates = sum(outcome["candidate_tables"] for outcome in geometry)
    if admitted and not any(outcome["admitted_intervals"] for outcome in geometry):
        return "table_admitted_no_intervals"
    if candidates and not admitted:
        return "candidate_table_under_mapped"
    if candidates:
        return "admitted_without_mapping"

    # No candidate descriptor table at all -- the real frontier for these ROMs.
    #
    # Wrapper-candidate rejection is deliberately NOT reported here. Measured
    # across the corpus it is the normal state even for ROMs that fully
    # succeed: Mega Man 64 rejects 631 of 632 wrapper candidates and still
    # recovers 28 banks through the descriptor-table path. Treating it as a
    # failure reason made a routine non-event look like the dominant frontier.
    unprovable = sum(outcome["wrapper_semantic_proof_unavailable"] for outcome in geometry)
    if unprovable:
        return "wrapper_shape_awaiting_proof"
    return "no_candidate_table_found"


def classify(record: dict[str, Any]) -> str:
    """Why a ROM is where it is -- the bucket that decides what to fix."""
    if record["boot_entropy"] >= COMPRESSED_ENTROPY_FLOOR:
        return "compressed_boot"
    if record["loader_stub_ratio"] >= RESIDENT_STUB_RATIO_CEILING:
        return "loader_stub"
    if record["code_run_share"] < 0.5:
        return "sparse_boot"
    return "resident_code"


def validate_owner_proof(value: Any, digest: str) -> dict[str, Any]:
    """Validate the structured Rust receipt without changing its vocabulary."""
    if not isinstance(value, dict):
        raise FrontierError(f"{digest} owner_proof is not an object")
    if value.get("coverage_blocker_payloads_omitted") is not True:
        raise FrontierError(
            f"{digest} owner_proof must declare omitted coverage blocker payloads"
        )

    def require_object(item: Any, label: str) -> dict[str, Any]:
        if not isinstance(item, dict):
            raise FrontierError(f"{digest} owner_proof {label} is not an object")
        return item

    def require_list(item: dict[str, Any], field: str, label: str) -> list[Any]:
        held = item.get(field)
        if not isinstance(held, list):
            raise FrontierError(f"{digest} owner_proof {label}.{field} is not an array")
        return held

    def require_count(item: dict[str, Any], field: str, label: str) -> None:
        held = item.get(field)
        if not isinstance(held, int) or isinstance(held, bool) or held < 0:
            raise FrontierError(
                f"{digest} owner_proof {label}.{field} is not a nonnegative integer"
            )

    def validate_marginals(items: list[Any], label: str) -> None:
        for index, raw in enumerate(items):
            item_label = f"{label}[{index}]"
            item = require_object(raw, item_label)
            if not isinstance(item.get("kind"), str) or not item["kind"]:
                raise FrontierError(f"{digest} owner_proof {item_label}.kind is invalid")
            for field in (
                "affected_assessments",
                "occurrences",
                "sole_blocker_assessments",
            ):
                require_count(item, field, item_label)

    def validate_combinations(items: list[Any], label: str) -> None:
        for index, raw in enumerate(items):
            item_label = f"{label}[{index}]"
            item = require_object(raw, item_label)
            if item.get("assessment_state") not in ("candidate", "ambiguous"):
                raise FrontierError(
                    f"{digest} owner_proof {item_label}.assessment_state is invalid"
                )
            kinds = require_list(item, "kinds", item_label)
            if (
                not all(isinstance(kind, str) and kind for kind in kinds)
                or kinds != sorted(set(kinds))
            ):
                raise FrontierError(
                    f"{digest} owner_proof {item_label}.kinds is not sorted and unique"
                )
            require_count(item, "assessments", item_label)

    def validate_ranges(raw: Any, label: str) -> None:
        item = require_object(raw, label)
        require_count(item, "proven_range_count", label)
        require_count(item, "proven_bytes", label)
        for index, raw_provenance in enumerate(require_list(item, "provenance", label)):
            provenance_label = f"{label}.provenance[{index}]"
            provenance = require_object(raw_provenance, provenance_label)
            if not isinstance(provenance.get("rule"), str) or not provenance["rule"]:
                raise FrontierError(
                    f"{digest} owner_proof {provenance_label}.rule is invalid"
                )
            require_count(provenance, "range_count", provenance_label)
            require_count(provenance, "bytes", provenance_label)

    def validate_indirect(raw: Any, label: str) -> None:
        item = require_object(raw, label)
        for field in (
            "total_sites",
            "exhaustive_sites",
            "bounded_sites",
            "open_sites",
            "via_call_sites",
            "via_jump_sites",
        ):
            require_count(item, field, label)
        for index, raw_kind in enumerate(require_list(item, "resolution_kinds", label)):
            kind_label = f"{label}.resolution_kinds[{index}]"
            kind = require_object(raw_kind, kind_label)
            if kind.get("kind") not in (
                "constant",
                "memory_value_set",
                "jump_table",
                "unresolved",
            ):
                raise FrontierError(f"{digest} owner_proof {kind_label}.kind is invalid")
            require_count(kind, "sites", kind_label)
        for index, raw_count in enumerate(
            require_list(item, "target_count_distribution", label)
        ):
            count_label = f"{label}.target_count_distribution[{index}]"
            count = require_object(raw_count, count_label)
            require_count(count, "target_count", count_label)
            require_count(count, "sites", count_label)

    validate_marginals(require_list(value, "blocker_marginals", "root"), "blocker_marginals")
    root_combinations = require_list(value, "blocker_combinations", "root")
    validate_combinations(root_combinations, "blocker_combinations")
    validate_ranges(value.get("executable_ranges"), "executable_ranges")
    validate_indirect(value.get("indirect_transfers"), "indirect_transfers")
    unresolved_assessments = 0
    for index, raw_bank in enumerate(require_list(value, "banks", "root")):
        label = f"banks[{index}]"
        bank = require_object(raw_bank, label)
        if not isinstance(bank.get("bank"), str) or not bank["bank"]:
            raise FrontierError(f"{digest} owner_proof {label}.bank is invalid")
        for field in (
            "assessed_entries",
            "exact_owners",
            "candidate_owners",
            "ambiguous_owners",
            "exact_owner_bytes",
        ):
            require_count(bank, field, label)
        validate_marginals(
            require_list(bank, "blocker_marginals", label), f"{label}.blocker_marginals"
        )
        if bank["assessed_entries"] != (
            bank["exact_owners"] + bank["candidate_owners"] + bank["ambiguous_owners"]
        ):
            raise FrontierError(f"{digest} owner_proof {label} owner totals do not balance")
        bank_combinations = require_list(bank, "blocker_combinations", label)
        validate_combinations(bank_combinations, f"{label}.blocker_combinations")
        bank_unresolved = bank["candidate_owners"] + bank["ambiguous_owners"]
        if sum(item["assessments"] for item in bank_combinations) != bank_unresolved:
            raise FrontierError(
                f"{digest} owner_proof {label} blocker combinations do not cover unresolved owners"
            )
        unresolved_assessments += bank_unresolved
        validate_ranges(bank.get("executable_ranges"), f"{label}.executable_ranges")
        validate_indirect(bank.get("indirect_transfers"), f"{label}.indirect_transfers")
    if sum(item["assessments"] for item in root_combinations) != unresolved_assessments:
        raise FrontierError(
            f"{digest} owner_proof aggregate blocker combinations do not match bank rows"
        )
    return value


def join(
    catalog: list[dict[str, Any]],
    summaries: dict[str, dict[str, Any]],
    measurements: dict[str, dict[str, Any]] | None = None,
) -> list[dict[str, Any]]:
    rows = []
    for record in catalog:
        digest = record["normalized_rom_sha256"]
        summary = summaries.get(digest)
        if summary is None:
            continue
        coverage = summary["coverage"]
        states = coverage["function_entries_by_state"]
        proven = states.get("Proven", 0)
        targets = record["distinct_jal_targets"]
        outcomes = summary.get("strategy_outcomes", [])
        geometry = {
            outcome["strategy"]: outcome
            for outcome in outcomes
            if outcome["strategy"] in GEOMETRY_STRATEGIES
        }
        row = {
                "schema": FRONTIER_SCHEMA,
                "normalized_rom_sha256": digest,
                "stable_id": record.get("stable_id", ""),
                "internal_name": record["internal_name"],
                "ipl3_group": record["ipl3_group"],
                "developer": record.get("developer", ""),
                "class": classify(record),
                "geometry_failure": geometry_failure(outcomes),
                "candidate_tables": sum(o["candidate_tables"] for o in geometry.values()),
                "admitted_tables": sum(o["admitted_tables"] for o in geometry.values()),
                "admitted_intervals": sum(o["admitted_intervals"] for o in geometry.values()),
                "wrapper_candidates_examined": sum(
                    o["physical_wrapper_candidates_examined"] for o in geometry.values()
                ),
                "wrapper_shape_rejections": {
                    name: sum(
                        o.get("wrapper_shape_rejections", {}).get(name, 0)
                        for o in geometry.values()
                    )
                    for name in (
                        "no_end_minus_start",
                        "no_nested_dma_call",
                        "destination_not_advanced",
                        "physical_not_advanced",
                        "remaining_not_reduced",
                        "no_backward_loop",
                        "no_return",
                    )
                },
                "selected_strategy": summary["selected_strategy"],
                "mapped_banks": coverage["mapped_banks"],
                "executable_bytes": coverage["executable_bytes"],
                "proven_entries": proven,
                "candidate_entries": states.get("Candidate", 0),
                "supported_entries": states.get("Supported", 0),
                "distinct_jal_targets": targets,
                # The magnitude of the gap, not merely that one exists.
                "proven_target_share": round(proven / targets, 6) if targets else 0.0,
                "loader_stub_ratio": record["loader_stub_ratio"],
                "code_run_share": record["code_run_share"],
                "boot_entropy": record["boot_entropy"],
                "unaligned_mem": record["unaligned_mem"],
                "cache_ops": record["cache_ops"],
                "branch_likely": record["branch_likely"],
            }
        # Optional additions leave old summary fixtures and v1 receipts
        # readable while retaining the structured proof data when requested.
        if "owner_proof" in summary:
            row["owner_proof"] = validate_owner_proof(summary["owner_proof"], digest)
        if measurements is not None and digest in measurements:
            measurement = measurements[digest]
            row["wall_seconds"] = measurement["wall_seconds"]
            row["sampled_peak_rss_bytes"] = measurement["sampled_peak_rss_bytes"]
            row["rss_scope"] = "direct_process"
            row["rss_sample_interval_seconds"] = RSS_SAMPLE_INTERVAL_SECONDS
            row["discovery_binary_sha256"] = measurement["discovery_binary_sha256"]
        rows.append(row)
    if not rows:
        raise FrontierError("no catalog record joined a discovery summary")
    return rows


def histogram(rows: list[dict[str, Any]], field: str) -> list[tuple[Any, int]]:
    counts = collections.Counter(row[field] for row in rows)
    return sorted(counts.items(), key=lambda item: (-item[1], str(item[0])))


def report(rows: list[dict[str, Any]]) -> None:
    """Aggregate tables to stdout. Per-ROM detail belongs in --output."""
    total = len(rows)
    print(f"frontier over {total} ROMs\n")

    print("selected strategy:")
    for value, count in histogram(rows, "selected_strategy"):
        print(f"  {value:<24}{count:>5}")

    print("\nboot-bank class:")
    for value, count in histogram(rows, "class"):
        print(f"  {value:<24}{count:>5}")

    no_exec = [row for row in rows if row["executable_bytes"] == 0]
    single_bank = [row for row in rows if row["mapped_banks"] <= 1]
    print(f"\nexecutable_bytes == 0:      {len(no_exec):>5} of {total}")
    print(f"mapped_banks <= 1:          {len(single_bank):>5} of {total}")

    print("\nhazard usage (ROMs with a nonzero count):")
    for field in ("unaligned_mem", "cache_ops", "branch_likely"):
        using = sum(1 for row in rows if row[field] > 0)
        busiest = max(rows, key=lambda row: row[field])
        print(
            f"  {field:<16}{using:>5} of {total}"
            f"   max {busiest[field]} ({busiest['internal_name']})"
        )

    # Load geometry is the bottleneck -- ROMs that recover it harvest roughly
    # ten times the rest -- so the actionable output is which proof condition
    # went unmet, not a difficulty score. Boot-bank measures do not predict
    # harvest (code_run_share correlates at r=+0.14), so they are reported as
    # classifiers only and never used to rank.
    print("\ngeometry failure reason:")
    for value, count in histogram(rows, "geometry_failure"):
        print(f"  {value:<32}{count:>5}")

    recovered = [row for row in rows if row["geometry_failure"] == "recovered"]
    print(f"\nrecovered load geometry ({len(recovered)} ROMs):")
    print(f"  {'internal name':<28}{'banks':>6}{'tables':>7}{'intervals':>10}  developer")
    for row in sorted(recovered, key=lambda row: -row["mapped_banks"]):
        print(
            f"  {row['internal_name'][:27]:<28}"
            f"{row['mapped_banks']:>6}"
            f"{row['admitted_tables']:>7}"
            f"{row['admitted_intervals']:>10}"
            f"  {row['developer']}"
        )

    # A candidate found and then rejected is a proof-rule gap: the detector
    # works and the admission bar is what stands in the way. That is a far
    # smaller, more specific problem than finding no candidate at all.
    # Reported separately from the failure histogram: wrapper rejection is not
    # a failure reason (ROMs that recover geometry reject them too), but which
    # fact fails first still says where the detector's reach ends.
    facts = collections.Counter()
    examined = 0
    for row in rows:
        examined += row["wrapper_candidates_examined"]
        for name, count in row["wrapper_shape_rejections"].items():
            facts[name] += count
    if examined:
        print(f"\nwrapper candidates examined corpus-wide: {examined}")
        print("  rejected for (facts cascade, so one candidate counts under several):")
        for name, count in facts.most_common():
            print(f"    {name:<28}{count:>9}")

    near = [row for row in rows if row["candidate_tables"] and row["mapped_banks"] <= 1]
    near.sort(key=lambda row: -row["candidate_tables"])
    print(f"\ncandidate tables found but not mapped ({len(near)} ROMs):")
    for row in near[:15]:
        print(
            f"  {row['internal_name'][:27]:<28}"
            f"cand={row['candidate_tables']:<5}"
            f"admitted={row['admitted_tables']:<5}"
            f"{row['geometry_failure']}"
        )

    owner_rows = [row for row in rows if "owner_proof" in row]
    if owner_rows:
        assessed = exact = candidate = ambiguous = exact_bytes = 0
        marginals: dict[str, list[int]] = {}
        combinations: collections.Counter[tuple[str, tuple[str, ...]]] = collections.Counter()
        indirect = collections.Counter()
        for row in owner_rows:
            proof = row["owner_proof"]
            for bank in proof["banks"]:
                assessed += bank["assessed_entries"]
                exact += bank["exact_owners"]
                candidate += bank["candidate_owners"]
                ambiguous += bank["ambiguous_owners"]
                exact_bytes += bank["exact_owner_bytes"]
            for marginal in proof["blocker_marginals"]:
                counts = marginals.setdefault(marginal["kind"], [0, 0, 0])
                counts[0] += marginal["affected_assessments"]
                counts[1] += marginal["sole_blocker_assessments"]
                counts[2] += marginal["occurrences"]
            for combination in proof["blocker_combinations"]:
                key = (combination["assessment_state"], tuple(combination["kinds"]))
                combinations[key] += combination["assessments"]
            for field in (
                "total_sites",
                "exhaustive_sites",
                "bounded_sites",
                "open_sites",
                "via_call_sites",
                "via_jump_sites",
            ):
                indirect[field] += proof["indirect_transfers"][field]

        print(f"\nowner proof ({len(owner_rows)} ROMs):")
        print(
            f"  assessed={assessed} exact={exact} exact_bytes={exact_bytes} "
            f"candidate={candidate} ambiguous={ambiguous}"
        )
        print("  blocker kinds (affected assessments, sole immediate payoff, site occurrences):")
        for kind, counts in sorted(marginals.items(), key=lambda item: (-item[1][0], item[0])):
            print(f"    {kind:<38}{counts[0]:>10}{counts[1]:>10}{counts[2]:>14}")
        print("  dominant exact blocker combinations:")
        for (state, kinds), count in combinations.most_common(12):
            print(f"    {state:<10}{count:>10}  {' + '.join(kinds)}")
        print(
            "  indirect sites: "
            f"total={indirect['total_sites']} exhaustive={indirect['exhaustive_sites']} "
            f"bounded={indirect['bounded_sites']} open={indirect['open_sites']} "
            f"calls={indirect['via_call_sites']} jumps={indirect['via_jump_sites']}"
        )


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--catalog", required=True, help="rom-catalog.py JSONL")
    result.add_argument("--binary", required=True, help="fn64-discover executable")
    result.add_argument(
        "--rom-dir",
        help="ROM directory; defaults to $FN64_ROM_CORPUS_DIR. No relative fallback.",
    )
    result.add_argument("--output", help="absolute JSONL path for per-ROM detail")
    result.add_argument(
        "--failures-output",
        help="absolute JSONL path for path-free failed-ROM diagnostics",
    )
    result.add_argument("--timeout-seconds", type=int, default=600)
    result.add_argument(
        "--jobs",
        type=int,
        default=min(2, (os.cpu_count() or 1)),
        help="concurrent owner-proof subprocesses (default: 2; raise explicitly after measuring RSS)",
    )
    result.add_argument("--limit", type=int, help="stop after N ROMs (smoke runs)")
    return result


def main() -> int:
    try:
        args = parser().parse_args()
        rom_dir_text = args.rom_dir or os.environ.get("FN64_ROM_CORPUS_DIR")
        if not rom_dir_text:
            raise FrontierError(
                "set --rom-dir or FN64_ROM_CORPUS_DIR; there is no default ROM location"
            )
        rom_dir = Path(rom_dir_text)
        binary = Path(args.binary)
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise FrontierError(f"{binary} is not an executable file")
        if args.jobs <= 0:
            raise FrontierError("jobs must be positive")
        binary_sha256 = sha256_file(binary)
        output_path = validate_output_destination(args.output) if args.output else None
        failures_path = (
            validate_output_destination(args.failures_output) if args.failures_output else None
        )
        if output_path is not None and failures_path == output_path:
            raise FrontierError("output and failures-output must be different files")

        catalog = load_catalog(Path(args.catalog))
        if args.limit is not None:
            catalog = catalog[: args.limit]
        catalog_digests = {record["normalized_rom_sha256"] for record in catalog}

        rom_paths = [
            path
            for path in sorted(rom_dir.iterdir())
            if path.suffix.lower() in (".z64", ".n64", ".v64")
        ]
        if args.limit is not None:
            rom_paths = rom_paths[: args.limit]

        # Each ROM is independent, but snapshot composition retains materially
        # more memory than plain discovery. Keep the default bounded; an
        # operator can raise it after the sampled RSS field demonstrates room.
        summaries: dict[str, dict[str, Any]] = {}
        measurements: dict[str, dict[str, Any]] = {}
        failures: list[dict[str, Any]] = []
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
            pending = {
                pool.submit(run_discovery, binary, path, args.timeout_seconds): (index, path)
                for index, path in enumerate(rom_paths)
            }
            for future in concurrent.futures.as_completed(pending):
                try:
                    result = future.result()
                except FrontierError as error:
                    # One unreadable ROM must not discard the whole sweep, but
                    # it is still reported rather than silently dropped.
                    failures.append(
                        {
                            "schema": FAILURE_SCHEMA,
                            "input_index": pending[future][0],
                            "input_name": pending[future][1].name,
                            "identity_scope": "path_free_basename_not_verified_digest",
                            "error": str(error),
                        }
                    )
                    continue
                summary = result["summary"]
                digest = summary.get("normalized_rom_sha256")
                if digest not in catalog_digests or digest in summaries:
                    failures.append(
                        {
                            "schema": FAILURE_SCHEMA,
                            "input_index": pending[future][0],
                            "input_name": pending[future][1].name,
                            "identity_scope": "path_free_basename_not_verified_digest",
                            "error": (
                                f"{pending[future][1].name} summary digest is not a unique "
                                "catalog member"
                            ),
                        }
                    )
                    continue
                summaries[digest] = summary
                measurements[digest] = {
                    "wall_seconds": result["wall_seconds"],
                    "sampled_peak_rss_bytes": result["peak_rss_bytes"],
                    "discovery_binary_sha256": binary_sha256,
                }
        failures.sort(key=lambda failure: failure["input_name"])
        for failure in failures:
            print(f"rom-frontier: {failure['error']}", file=sys.stderr)
        if not summaries:
            if failures_path is not None:
                publish_records(failures_path, failures)
            raise FrontierError("no ROM completed discovery")

        rows = join(catalog, summaries, measurements)
        report(rows)
        if output_path is not None:
            publish_records(output_path, rows)
        if failures_path is not None:
            publish_records(failures_path, failures)
        return 0
    except FrontierError as error:
        print(f"rom-frontier: {error}", file=sys.stderr)
        return 1
    except OSError as error:
        print(f"rom-frontier: operating-system error ({error.errno})", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
