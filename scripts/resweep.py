#!/usr/bin/env python3
"""Re-measure open-indirect shapes and counterfactuals across all 287 ROMs.

Every prior corpus figure excluded 36 ROMs (13%) that failed
TransferOpcodeMismatch -- including Ocarina of Time, Kirby 64 and Mario Party,
three of the six graded answer-key games. Codex fixed the classifier; this
re-runs against complete data.
"""
import concurrent.futures, glob, json, os, subprocess, sys

BIN = "/Users/jer/Code/fn64/.claude/worktrees/rom-corpus-catalog/target/release/diagnose_open_indirects"

def one(path):
    name = os.path.basename(path)
    try:
        r = subprocess.run([BIN, path], capture_output=True, timeout=900)
    except subprocess.TimeoutExpired:
        return {"rom": name, "error": "timeout"}
    line = next((l for l in r.stdout.decode("utf-8", "replace").splitlines()
                 if l.startswith("{")), None)
    if line is None:
        err = r.stderr.decode("utf-8", "replace").strip().splitlines()
        return {"rom": name, "error": err[-1] if err else f"exit {r.returncode}"}
    d = json.loads(line)
    f = d["frontier"]
    return {
        "rom": name, "name": d["internal_name"], "sha": d["normalized_rom_sha256"][:12],
        "banks": d["banks"], "ms": d["elapsed_ms"], "open": f["open_sites"],
        "semantic_shapes": f.get("semantic_shapes", []),
        "counterfactuals": f.get("mechanism_counterfactuals", []),
    }

def main():
    out_path = sys.argv[1]
    files = sorted(glob.glob("/Users/jer/Code/roms/n64/*.z64"))
    res = []
    # Checkpoint every batch: the unbatched form died twice today writing nothing.
    for i in range(0, len(files), 40):
        with concurrent.futures.ThreadPoolExecutor(max_workers=10) as pool:
            for r in pool.map(one, files[i:i + 40]):
                res.append(r)
        json.dump(res, open(out_path, "w"), separators=(",", ":"))
        print(f"  {len(res)}/{len(files)}", file=sys.stderr, flush=True)
    ok = [r for r in res if "error" not in r]
    print(f"DONE {len(res)} measured, {len(ok)} classified, {len(res)-len(ok)} errors",
          file=sys.stderr)

main()
