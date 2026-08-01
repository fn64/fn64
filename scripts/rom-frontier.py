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
        rows.append(
            {
                "schema": FRONTIER_SCHEMA,
                "normalized_rom_sha256": digest,
                "stable_id": record.get("stable_id", ""),
                "internal_name": record["internal_name"],
                "ipl3_group": record["ipl3_group"],
                "developer": record.get("developer", ""),
                "class": classify(record),
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

    # The practical output: resident-code ROMs discovery still fails on need no
    # decompression and no streaming, so they are the closest to success.
    ranked = [
        row
        for row in rows
        if row["class"] == "resident_code" and row["proven_entries"] <= 1
    ]
    ranked.sort(key=lambda row: (-row["code_run_share"], row["loader_stub_ratio"]))
    print(f"\neasiest next targets ({len(ranked)} resident-code ROMs still unproven):")
    print(f"  {'internal name':<24}{'code%':>7}{'stub':>7}{'targets':>9}  developer")
    for row in ranked[:15]:
        print(
            f"  {row['internal_name'][:23]:<24}"
            f"{row['code_run_share'] * 100:>6.1f}%"
            f"{row['loader_stub_ratio']:>7.2f}"
            f"{row['distinct_jal_targets']:>9}"
            f"  {row['developer']}"
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
