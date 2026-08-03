#!/usr/bin/env python3
"""Re-measure open-indirect shapes and counterfactuals across all 287 ROMs.

Every prior corpus figure excluded 36 ROMs (13%) that failed
TransferOpcodeMismatch -- including Ocarina of Time, Kirby 64 and Mario Party,
three of the six graded answer-key games. Codex fixed the classifier; this
re-runs against complete data.
"""
import argparse
import concurrent.futures
import glob
import json
import os
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_BIN = os.path.join(REPO, "target", "release", "diagnose_open_indirects")

def one(binary, path, timeout):
    name = os.path.basename(path)
    try:
        r = subprocess.run(
            [binary, path], capture_output=True, text=True, timeout=timeout
        )
    except subprocess.TimeoutExpired:
        return {"rom": name, "error": "timeout"}
    line = next((line for line in r.stdout.splitlines() if line.startswith("{")), None)
    if line is None:
        prefix = f"diagnose-open-indirects: {path}: "
        reason = next(
            (line.removeprefix(prefix) for line in r.stderr.splitlines()
             if line.startswith(prefix)),
            None,
        )
        return {"rom": name, "error": reason or f"exit {r.returncode}"}
    d = json.loads(line)
    if d.get("schema") != "fn64.open-indirect-frontier.v2":
        return {"rom": name, "error": f"unexpected schema {d.get('schema')!r}"}
    f = d["frontier"]
    return {
        "rom": name, "name": d["internal_name"], "sha": d["normalized_rom_sha256"][:12],
        "banks": d["banks"], "ms": d["elapsed_ms"], "open": f["open_sites"],
        "owner_proof_open": d["owner_proof_frontier"]["open_sites"],
        "assessed": d["assessed_entries"], "exact": d["exact_owners"],
        "semantic_shapes": f.get("semantic_shapes", []),
        "counterfactuals": f.get("mechanism_counterfactuals", []),
        "owner_proof_semantic_shapes": d["owner_proof_frontier"].get("semantic_shapes", []),
        "owner_proof_counterfactuals": d["owner_proof_frontier"].get(
            "mechanism_counterfactuals", []),
    }

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("output")
    parser.add_argument("--bin", default=DEFAULT_BIN)
    parser.add_argument("--rom-dir", default="/Users/jer/Code/roms/n64")
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--batch-size", type=int, default=40)
    # WWF No Mercy exceeded the original hardcoded 900s under a loaded
    # 4-worker sweep; the cap must be raisable without editing the script.
    parser.add_argument("--timeout", type=float, default=900)
    args = parser.parse_args()
    if args.workers < 1:
        parser.error("--workers must be at least 1")
    if args.batch_size < 1:
        parser.error("--batch-size must be at least 1")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    if not os.path.isfile(args.bin):
        parser.error(f"diagnostic binary not found: {args.bin}")
    out_path = args.output
    files = sorted(glob.glob(os.path.join(args.rom_dir, "*.z64")))
    if not files:
        parser.error(f"no .z64 inputs found in {args.rom_dir}")
    started = time.monotonic()
    res = []
    # Checkpoint every batch: the unbatched form died twice today writing nothing.
    for i in range(0, len(files), args.batch_size):
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
            for r in pool.map(lambda path: one(args.bin, path, args.timeout), files[i:i + args.batch_size]):
                res.append(r)
        temporary = f"{out_path}.tmp"
        with open(temporary, "w") as output:
            json.dump(res, output, separators=(",", ":"))
        os.replace(temporary, out_path)
        print(f"  {len(res)}/{len(files)}", file=sys.stderr, flush=True)
    ok = [r for r in res if "error" not in r]
    elapsed = time.monotonic() - started
    print(
        f"DONE {len(res)} measured, {len(ok)} classified, "
        f"{len(res)-len(ok)} errors, elapsed={elapsed:.1f}s",
        file=sys.stderr,
    )

if __name__ == "__main__":
    main()
