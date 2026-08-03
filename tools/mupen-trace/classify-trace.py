#!/usr/bin/env python3
"""Classify a JSONL Mupen trace before it enters discovery evidence."""
import argparse
import json
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("trace", type=Path)
    ap.add_argument("--max-frontier-pcs", type=int, default=16)
    ap.add_argument("--min-records", type=int, default=64)
    args = ap.parse_args()
    rows = [json.loads(line) for line in args.trace.read_text().splitlines() if line.strip()]
    pcs = [json.dumps(row["pc"], sort_keys=True) for row in rows if row.get("event") == "executed_pc"]
    unique = list(dict.fromkeys(pcs))
    result = {
        "schema": "fn64.mupen-trace-classification.v1",
        "trace": str(args.trace),
        "executed_records": len(pcs),
        "unique_pcs": len(unique),
        "classification": "insufficient-observation",
    }
    if len(pcs) >= args.min_records and len(unique) <= args.max_frontier_pcs:
        result["classification"] = "device-progress-frontier"
        result["reason"] = "small repeating pause-PC set; inspect asynchronous device progress"
    elif pcs:
        result["classification"] = "diverse-execution-observation"
    print(json.dumps(result, sort_keys=True))
    return 2 if result["classification"] == "device-progress-frontier" else 0


if __name__ == "__main__":
    raise SystemExit(main())
