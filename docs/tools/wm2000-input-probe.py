#!/usr/bin/env python3
"""Generate + evaluate a WM2000 plateau button-probe matrix.

The plateau: three independent navigation strategies reach the same screen
around VI swap ~2500 and never leave it (docs/RT64-WM2000-GAMEPLAY-GAP.md
section 5). This script drives the harness's `WM2000_INPUT_SCRIPT` env var
over a matrix of single buttons and short combos applied AT the plateau, and
diffs the resulting per-swap frame hashes against a control run that presses
nothing new. A probe that produces frame hashes the control never produces is
a candidate for the grammar the screen wants.

Frame hashes come from the harness's own PNG dumps. The dumps are written at
a hardcoded 320x240 while WM2000 actually scans out 480x237 -- that shears the
image but it is a PURE FUNCTION of the same RDRAM bytes, so hashing the PNG
is still a sound equality test between runs. We never interpret a dumped PNG
as a picture here; for pictures, re-render at the true geometry.
"""
import argparse, hashlib, os, pathlib, subprocess, sys

# N64 OSContPad button bits.
BTN = {
    "A": 0x8000, "B": 0x4000, "Z": 0x2000, "START": 0x1000,
    "DUP": 0x0800, "DDOWN": 0x0400, "DLEFT": 0x0200, "DRIGHT": 0x0100,
    "L": 0x0020, "R": 0x0010,
    "CUP": 0x0008, "CDOWN": 0x0004, "CLEFT": 0x0002, "CRIGHT": 0x0001,
}

def frame_hashes(dump_dir):
    """swap index -> sha256 of the dumped framebuffer PNG."""
    out = {}
    for p in pathlib.Path(dump_dir).glob("fn64-fb-*.png"):
        try:
            swap = int(p.stem.rsplit("-", 1)[1])
        except ValueError:
            continue
        out[swap] = hashlib.sha256(p.read_bytes()).hexdigest()[:16]
    return out

def prefix_script(lead_swap):
    """The proven 18-screen lead-in: START at 1100, then A every 100 swaps.

    Reproduced from docs/RT64-WM2000-GAMEPLAY-GAP.md section 3.2 -- this is the
    sequence already shown to reach the plateau screen, so every probe starts
    from the same guest state.
    """
    parts = ["1100..1110:1000"]
    swap = 1200
    while swap < lead_swap:
        parts.append(f"{swap}..{swap+10}:8000")
        swap += 100
    return parts

def probe_script(lead_swap, buttons, sx=0, sy=0, taps=6, gap=60, hold=10):
    """Lead-in, then repeated taps of `buttons` starting at the plateau."""
    parts = prefix_script(lead_swap)
    swap = lead_swap
    for _ in range(taps):
        e = f"{swap}..{swap+hold}:{buttons:04x}"
        if sx or sy:
            e += f":{sx}:{sy}"
        parts.append(e)
        swap += gap
    return ";".join(parts)

def run(label, script, out_root, max_steps, run_sh, fn64, scratch, env_extra=None):
    """Run the rs lane once with `script`; return (dump_dir, log_path, rc)."""
    dump = pathlib.Path(out_root) / label
    dump.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    env.update({
        "FN64": fn64,
        "SCRATCH": scratch,
        "WM2000_INPUT_SCRIPT": script,
        "WM2000_MAX_STEPS": str(max_steps),
        "WM2000_FB_DUMP_DIR": str(dump),
    })
    if env_extra:
        env.update(env_extra)
    log = pathlib.Path(out_root) / f"{label}.log"
    with open(log, "w") as fh:
        rc = subprocess.call([run_sh], env=env, stdout=fh, stderr=subprocess.STDOUT,
                             cwd=str(pathlib.Path(run_sh).parent))
    return dump, log, rc

def compare(control, probe):
    """Swaps where `probe` shows a hash the control run never produced anywhere."""
    seen = set(control.values())
    novel = {s: h for s, h in sorted(probe.items()) if h not in seen}
    return novel

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run-sh", required=True)
    ap.add_argument("--fn64", required=True)
    ap.add_argument("--scratch", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--lead-swap", type=int, default=2500,
                    help="VI swap the plateau screen is reached at")
    ap.add_argument("--max-steps", type=int, default=350000)
    ap.add_argument("--probes", default="",
                    help="comma list of button names/combos, e.g. B,Z,CUP,A+B; "
                         "default = the full single-button matrix")
    ap.add_argument("--sticks", action="store_true",
                    help="also probe the four analog stick directions")
    args = ap.parse_args()

    names = [p for p in args.probes.split(",") if p] or list(BTN)
    jobs = []
    for n in names:
        bits = 0
        for part in n.split("+"):
            bits |= BTN[part.strip().upper()]
        jobs.append((n, bits, 0, 0))
    if args.sticks:
        for n, sx, sy in (("STICK_L", -80, 0), ("STICK_R", 80, 0),
                          ("STICK_U", 0, 80), ("STICK_D", 0, -80)):
            jobs.append((n, 0, sx, sy))

    # Control: the lead-in alone, nothing pressed at the plateau. Every probe
    # is judged against THIS run's hash set, so "the screen changed on its own"
    # cannot be mistaken for "the button did something".
    print("== control (lead-in only, no plateau input) ==", flush=True)
    cd, _, rc = run("control", ";".join(prefix_script(args.lead_swap)),
                    args.out, args.max_steps, args.run_sh, args.fn64, args.scratch)
    control = frame_hashes(cd)
    print(f"control: rc={rc} {len(control)} frames, "
          f"{len(set(control.values()))} distinct hashes", flush=True)

    results = []
    for name, bits, sx, sy in jobs:
        script = probe_script(args.lead_swap, bits, sx, sy)
        d, _, rc = run(f"probe-{name}", script, args.out, args.max_steps,
                       args.run_sh, args.fn64, args.scratch)
        h = frame_hashes(d)
        novel = compare(control, h)
        first = min(novel) if novel else None
        results.append((name, rc, len(h), len(set(h.values())), len(novel), first))
        print(f"{name:10s} rc={rc} frames={len(h):5d} distinct={len(set(h.values())):3d} "
              f"NOVEL={len(novel):5d} first_novel_swap={first}", flush=True)

    print("\n== ranked by novel frames ==", flush=True)
    for r in sorted(results, key=lambda r: -r[4]):
        print(f"  {r[0]:10s} novel={r[4]:5d} first={r[5]}", flush=True)

if __name__ == "__main__":
    main()
