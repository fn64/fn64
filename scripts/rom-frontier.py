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
import json
import os
import secrets
import stat
import subprocess
import sys
from pathlib import Path
from typing import Any, Iterable


FRONTIER_SCHEMA = "fn64.rom-frontier.v1"
CATALOG_SCHEMA = "fn64.rom-catalog.v1"

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


def run_discovery(binary: Path, rom_path: Path, timeout_seconds: int) -> dict[str, Any]:
    """Run `fn64-discover <rom> --summary` and return its parsed summary."""
    try:
        completed = subprocess.run(
            [str(binary), str(rom_path), "--summary"],
            capture_output=True,
            timeout=timeout_seconds,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise FrontierError(f"{rom_path.name} exceeded {timeout_seconds}s") from error
    if completed.returncode != 0:
        raise FrontierError(f"{rom_path.name} discovery exited {completed.returncode}")
    for line in completed.stdout.decode("utf-8", "replace").splitlines():
        if line.startswith("{"):
            return json.loads(line)["summary"]
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


def join(catalog: list[dict[str, Any]], summaries: dict[str, dict[str, Any]]) -> list[dict[str, Any]]:
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
        rows.append(
            {
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
        )
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


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--catalog", required=True, help="rom-catalog.py JSONL")
    result.add_argument("--binary", required=True, help="fn64-discover executable")
    result.add_argument(
        "--rom-dir",
        help="ROM directory; defaults to $FN64_ROM_CORPUS_DIR. No relative fallback.",
    )
    result.add_argument("--output", help="absolute JSONL path for per-ROM detail")
    result.add_argument("--timeout-seconds", type=int, default=600)
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
        output_path = validate_output_destination(args.output) if args.output else None

        catalog = load_catalog(Path(args.catalog))
        if args.limit is not None:
            catalog = catalog[: args.limit]

        summaries: dict[str, dict[str, Any]] = {}
        for rom_path in sorted(rom_dir.iterdir()):
            if rom_path.suffix.lower() not in (".z64", ".n64", ".v64"):
                continue
            summary = run_discovery(binary, rom_path, args.timeout_seconds)
            summaries[summary["normalized_rom_sha256"]] = summary
            if args.limit is not None and len(summaries) >= args.limit:
                break

        rows = join(catalog, summaries)
        report(rows)
        if output_path is not None:
            publish_records(output_path, rows)
        return 0
    except FrontierError as error:
        print(f"rom-frontier: {error}", file=sys.stderr)
        return 1
    except OSError as error:
        print(f"rom-frontier: operating-system error ({error.errno})", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
