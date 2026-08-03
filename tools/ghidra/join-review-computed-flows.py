#!/usr/bin/env python3
"""Join candidate function inventory with candidate computed-flow claims.

This is a reporting-only operation. It never changes native facts or promotes
any candidate to authority.
"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path


SCHEMA = "fn64.n64loaderwv-computed-flow-join.v1"
MAX_INPUT_BYTES = 128 * 1024 * 1024


def read_bounded(path: Path) -> bytes:
    data = path.read_bytes()
    if len(data) > MAX_INPUT_BYTES:
        raise ValueError(f"input exceeds {MAX_INPUT_BYTES} bytes: {path}")
    return data


def contains(ranges: list[dict], address: int) -> bool:
    return any(item["start"] <= address < item["end_exclusive"] for item in ranges)


def main(argv: list[str]) -> int:
    if len(argv) != 4:
        print("usage: join-review-computed-flows.py INVENTORY PROVIDER_JSONL OUT", file=sys.stderr)
        return 2
    inventory_path, provider_path, output_path = map(Path, argv[1:])
    try:
        inventory_bytes = read_bounded(inventory_path)
        inventory = json.loads(inventory_bytes)
        if inventory.get("candidate_only") is not True:
            raise ValueError("inventory is not candidate-only")
        functions = inventory.get("functions")
        if not isinstance(functions, list) or not functions:
            raise ValueError("inventory has no functions")
        provider_bytes = read_bounded(provider_path)
        claims = []
        for line_number, line in enumerate(provider_bytes.decode("utf-8").splitlines(), 1):
            if not line:
                continue
            record = json.loads(line)
            if record.get("record") != "claim":
                continue
            claim = record.get("claim", {})
            if claim.get("type") != "computed_control_flow":
                continue
            site = claim.get("site", {})
            targets = claim.get("targets", [])
            if not isinstance(site.get("pc"), int) or not isinstance(targets, list):
                raise ValueError(f"invalid computed-flow claim at line {line_number}")
            if any(not isinstance(target.get("pc"), int) for target in targets):
                raise ValueError(f"invalid computed-flow target at line {line_number}")
            claims.append({
                "sequence": record.get("sequence"),
                "provider_claim_id": record.get("provider_claim_id"),
                "site": site["pc"],
                "via_call": claim.get("via_call"),
                "targets": [target["pc"] for target in targets],
            })
    except (OSError, UnicodeError, ValueError, json.JSONDecodeError) as error:
        print(f"join-review-computed-flows: {error}", file=sys.stderr)
        return 1

    def function_at(address: int):
        return next((f for f in functions if contains(f["body_ranges"], address)), None)

    joined = []
    for claim in claims:
        site_function = function_at(claim["site"])
        target_functions = []
        for target in claim["targets"]:
            function = function_at(target)
            target_functions.append({
                "pc": target,
                "function_entry": function["entry"] if function else None,
                "reachable_from_loader_entry": (
                    function.get("reachable_from_loader_entry", False) if function else False
                ),
            })
        joined.append({
            **claim,
            "site_function_entry": site_function["entry"] if site_function else None,
            "site_function_reachable_from_loader_entry": (
                site_function.get("reachable_from_loader_entry", False)
                if site_function else False
            ),
            "targets": target_functions,
        })

    body = {
        "candidate_only": True,
        "production_ingest_authority": False,
        "inventory_sha256": hashlib.sha256(inventory_bytes).hexdigest(),
        "provider_sha256": hashlib.sha256(provider_bytes).hexdigest(),
        "inventory_function_count": len(functions),
        "computed_flow_count": len(joined),
        "sites_without_function": sum(item["site_function_entry"] is None for item in joined),
        "claims_with_unreachable_target": sum(
            any(not target["reachable_from_loader_entry"] for target in item["targets"])
            for item in joined
        ),
        "claims": joined,
    }
    output = {"schema": SCHEMA, "body": body}
    output_path.write_text(json.dumps(output, sort_keys=True, separators=(",", ":")) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
