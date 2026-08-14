#!/bin/zsh
# Quantify the ROM-word contribution in the built binaries: overall size, Mach-O
# section sizes, and the between-lane delta. This replaces the ~1.94 MiB EXTENT
# CALCULATION in docs/plans/rom-content-in-shipped-artifact.md with a measurement.

set -uo pipefail
REPO=/Users/jer/Code/fn64/.claude/worktrees/rom-corpus-catalog
cd "$REPO" || exit 1

ON="$REPO/target-audit-verifyon/release/wm2000-block-boot"
OFF="$REPO/target-audit-verifyoff/release/wm2000-block-boot"

for b in "$ON" "$OFF"; do
    [[ -x "$b" ]] || { print -u2 "missing binary: $b"; exit 1; }
done

print "=== FILE SIZES"
stat -f "%z bytes  %N" "$ON" "$OFF"
python3 -c "
import os
a=os.path.getsize('$ON'); b=os.path.getsize('$OFF')
print(f'delta (verifyon - verifyoff) = {a-b} bytes = {(a-b)/2**20:.3f} MiB')
print(f'verifyoff is {100*(a-b)/a:.2f}% smaller than verifyon')
"

print ""
print "=== size(1) SEGMENT TOTALS"
for b in "$ON" "$OFF"; do print "-- $b"; size "$b"; done

print ""
print "=== SECTION SIZES, and the per-section delta"
python3 - "$ON" "$OFF" <<'PY'
import subprocess, sys, collections

def sections(path):
    out = subprocess.run(["otool","-l",path],capture_output=True,text=True).stdout
    secs, cur, in_s = {}, {}, False
    for line in out.splitlines():
        s=line.strip()
        if s.startswith("Section"): cur, in_s = {}, True
        elif not in_s: continue
        elif s.startswith("sectname "): cur["sect"]=s.split(None,1)[1]
        elif s.startswith("segname "):  cur["seg"]=s.split(None,1)[1]
        elif s.startswith("size "):
            v=s.split()[1]; cur["size"]=int(v,16) if v.startswith("0x") else int(v)
        elif s.startswith("offset "):
            if "sect" in cur and "size" in cur:
                secs[f"{cur.get('seg','?')},{cur['sect']}"]=cur["size"]
            cur, in_s = {}, False
    return secs

on, off = sections(sys.argv[1]), sections(sys.argv[2])
names = sorted(set(on)|set(off))
print(f"{'section':<28}{'verifyon':>14}{'verifyoff':>14}{'delta':>14}")
tot=0
for n in names:
    a,b = on.get(n,0), off.get(n,0)
    d=a-b; tot+=d
    mark = "   <<<" if abs(d) > 1024 else ""
    print(f"{n:<28}{a:>14,}{b:>14,}{d:>14,}{mark}")
print(f"{'TOTAL':<28}{sum(on.values()):>14,}{sum(off.values()):>14,}{tot:>14,}")
PY
