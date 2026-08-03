#!/usr/bin/env python3
"""Convert a path-free N64LoaderWV inventory into schema-v1 claims.

The result is candidate-only. It is a serialization bridge, not an authority
or a function-boundary validator.
"""

from __future__ import annotations

import hashlib
import json
import struct
import sys
from pathlib import Path


def put_string(h: "hashlib._Hash", value: str) -> None:
    encoded = value.encode()
    h.update(struct.pack("<Q", len(encoded)))
    h.update(encoded)


def claims_digest(bank: str, candidates: list[tuple[int, int, int, str]]) -> str:
    h = hashlib.sha256(b"fn64.tool-adapter.claim-records.v1\0")
    h.update(struct.pack("<Q", len(candidates)))
    for sequence, (tag, start, end, provider_id) in enumerate(candidates):
        h.update(struct.pack("<Q", sequence))
        put_string(h, provider_id)
        h.update(bytes([tag]))
        put_string(h, bank)
        h.update(struct.pack("<I", start))
        if tag == 2:
            h.update(struct.pack("<I", end))
    return h.hexdigest()


def main(argv: list[str]) -> int:
    if len(argv) != 13:
        print("usage: inventory-to-candidate-claims.py INVENTORY OUT BANK VA_START VA_END "
              "ROM_SHA BANK_SHA MAPPING_SHA LOADER_COMMIT EXTENSION_SHA CONFIG_SHA EVIDENCE_SHA",
              file=sys.stderr)
        return 2
    inventory_path, output_path = Path(argv[1]), Path(argv[2])
    bank, va_start, va_end = argv[3], int(argv[4], 0), int(argv[5], 0)
    rom_sha, bank_sha, mapping_sha = argv[6:9]
    loader_commit, extension_sha, config_sha, evidence_sha = argv[9:13]
    inventory = json.loads(inventory_path.read_text(encoding="utf-8"))
    if inventory.get("candidate_only") is not True:
        raise SystemExit("inventory is not candidate-only")
    functions = inventory.get("functions")
    if not isinstance(functions, list):
        raise SystemExit("inventory functions is not a list")
    candidates = []
    for function in functions:
        if function.get("block") != ".ram":
            continue
        start = int(function["entry"])
        end = int(function["body_envelope_end_exclusive"])
        if not (va_start <= start < end <= va_end and start % 4 == 0 and end % 4 == 0):
            raise SystemExit(f"function outside selected bank: {start:#x}")
        suffix = f"{start:08x}"
        candidates.extend([
            (1, start, 0, f"n64loaderwv:function-entry:{bank}:{suffix}"),
            (2, start, end, f"n64loaderwv:function-extent:{bank}:{suffix}"),
        ])
    candidates.sort(key=lambda item: (item[1], item[0], item[3]))
    claims_sha = claims_digest(bank, candidates)
    lines = [{
        "record": "header", "schema": "fn64.tool-adapter", "schema_version": 1,
        "tool": {"name": "n64loaderwv-first-contact", "version": loader_commit,
                 "build_sha256": extension_sha},
        "role": "function_boundary_candidates",
        "input": {"normalized_rom_sha256": rom_sha, "bank": bank,
                  "bank_bytes_sha256": bank_sha, "mapping_sha256": mapping_sha,
                  "va_start": va_start, "va_end": va_end},
        "lineage": [{"role": "tool_configuration", "source_sha256": config_sha},
                    {"role": "evidence_manifest", "source_sha256": evidence_sha}],
    }]
    for sequence, (tag, start, end, provider_id) in enumerate(candidates):
        claim = ({"type": "function_entry", "address": {"bank": bank, "pc": start}}
                 if tag == 1 else
                 {"type": "function_extent", "range": {"bank": bank,
                  "va_start": start, "va_end": end}})
        lines.append({"record": "claim", "sequence": sequence,
                      "provider_claim_id": provider_id, "claim": claim})
    lines.append({"record": "summary", "complete": True,
                  "analyzed_range": {"bank": bank, "va_start": va_start, "va_end": va_end},
                  "skipped_ranges": [], "claim_records": len(candidates),
                  "claims_sha256": claims_sha,
                  "resources": {"input_bytes": va_end - va_start, "elapsed_millis": 0,
                                "peak_memory_bytes": None, "limit_hit": False, "warnings": []}})
    output_path.write_text("\n".join(json.dumps(line, separators=(",", ":")) for line in lines) + "\n",
                          encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
