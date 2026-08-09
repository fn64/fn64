#!/bin/zsh

# WM2000 render benchmark: per-frame latency during SUSTAINED RENDERING.
#
# COMMITTED ON PURPOSE, for the same reason `entrance-to-match.schedule` is:
# a route recipe is evidence and belongs in the repository. The run that first
# reached WM2000's match-setup screen -- 3,917 graphics submits, 7,029 audio
# submits, idle_steps=0, render_error=None, 4,000 frames dumped -- existed only
# as prose in an agent's report. This script is that run, plus the measurement.
#
#
# WHAT PROBLEM THIS SOLVES
#
# The project's acceptance bar is "guaranteed 60fps": every frame under
# 16.667 ms. As of 2026-08-07 that bar had NO TEST.
#
# The standard benchmark route (`FN64_BLOCK_MAX_STEPS=19523`) reports
# `gfx_submits=0` and eight VI interrupts. It renders nothing. A p99 frame time
# cannot be measured on it at all -- see "Measuring the 60fps bar" in
# `docs/plans/perf-method.md`. Every
# frame-time figure quoted before this script was therefore either a windowed
# heartbeat over a 60-frame window that was immediately cleared, or an
# extrapolation from a route with no frames in it.
#
#
# THE TWO RATIOS, WHICH ARE NOT THE SAME NUMBER
#
# This is the documented measurement trap and the reason `frame_census`
# reports both, always:
#
#   RATIO A -- wall milliseconds per emulated VI field. Target 16.667.
#              THIS IS WHAT THE 60FPS BAR TESTS: a field is what a player sees.
#   RATIO B -- host wall time / guest virtual time. Target 1.000x.
#              This is the "times slower than the console" figure.
#
# They diverge whenever the guest does not emit fields at 60 Hz. WM2000 during
# boot emits at roughly 27 Hz, so the SAME run reads as ~2.4x on ratio A and
# ~1.1x on ratio B. Neither is wrong. Quoting one as if it were the other is,
# and has been done. The census prints both on adjacent lines so it cannot be.
#
#
# WHICH LANE, AND WHY
#
# HEADLESS (`wm2000-block-boot`), deliberately. Three reasons:
#
#   1. It isolates guest + runtime cost. The open question is whether the
#      EMULATION can hold a 16.667 ms budget; the windowed lane adds blit,
#      present, and window-system cost on top, which answers a different
#      question and cannot be subtracted back out afterwards.
#   2. It is reproducible without a display server, so this runs anywhere.
#   3. The windowed lane already reports p50/p95/p99/max per 60-frame heartbeat
#      (`2676139`), so the two are complementary rather than redundant.
#
# THE NUMBER THIS PRODUCES IS NOT A PLAYER-EXPERIENCED FRAME TIME. It excludes
# presentation. A windowed frame is this plus present cost, never less.
#
#
# THE STEADY-STATE WINDOW
#
# Boot and first-render are a transient: overlays activate, shards fault in,
# and the first fields can take hundreds of milliseconds. A p99 that includes
# them measures startup, not gameplay, and one such sample owns `max` forever.
#
# `FN64_FRAME_CENSUS_WARMUP_GFX` discards every field observed before the guest
# has submitted that many graphics tasks. Graphics submits are the gate rather
# than a step or field count because they are the direct evidence that the
# guest is rendering -- the transient ends exactly when they start climbing.
# The default below is deliberately past the first-render knee. Transient
# fields are still counted and reported separately; nothing is silently
# dropped.
#
#
# THE ROUTE IS DETERMINISTIC, WHICH IS WHAT MAKES THIS A BENCHMARK
#
# Two independent runs produced byte-identical `[fn64-heartbeat]` lines at every
# step checkpoint -- same `sim_time`, same `gfx_submits`, same `port0_reads`
# (452/1240/1934 submits at steps 200k/400k/600k). The guest work is fixed, so
# a difference between two runs of this script is a difference in HOST cost,
# which is exactly the property an A/B needs. Anything that changes those
# heartbeat counters changed the emulated program, not its speed, and
# invalidates the comparison.
#
# Usage:
#   reference/wm2000-routes/render-benchmark.zsh
#   reference/wm2000-routes/render-benchmark.zsh --steps 3000000
#   reference/wm2000-routes/render-benchmark.zsh --warmup-gfx 1000
#   reference/wm2000-routes/render-benchmark.zsh --max-load 8   # relax gate
#   reference/wm2000-routes/render-benchmark.zsh --binary /path/to/wm2000-block-boot
#
# Output: the `[frame-census]` lines, which are the deliverable.

set -uo pipefail

STEPS=1500000
# Picked from the measured submit trajectory, not guessed. Per 50,000 steps the
# guest submits 0, then 106, then 175 / 171 / 261 / ~175 each thereafter:
# nothing renders for the first 50k steps, and the rate reaches its steady
# ~175/50k and stays. Submit 300 is just past that knee, inside the stable
# regime. See "Measuring the 60fps bar" in docs/plans/perf-method.md.
WARMUP_GFX=300
MAX_LOAD=3.0
HEARTBEAT=200000
BINARY=""
DUMP_DIR=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --steps)      STEPS="$2"; shift 2 ;;
        --warmup-gfx) WARMUP_GFX="$2"; shift 2 ;;
        --max-load)   MAX_LOAD="$2"; shift 2 ;;
        --heartbeat)  HEARTBEAT="$2"; shift 2 ;;
        --binary)     BINARY="$2"; shift 2 ;;
        --dump-dir)   DUMP_DIR="$2"; shift 2 ;;
        -h|--help)    sed -n '1,80p' "$0"; exit 0 ;;
        *) print -u2 "unknown argument: $1"; exit 2 ;;
    esac
done

HERE="${0:A:h}"
REPO="${HERE:h:h}"
cd "$REPO" || exit 1

# --- Rule 4 of docs/plans/perf-method.md: only measure on a quiet machine.
#
# This gate is not boilerplate. A concurrent 32-crate shard rebuild made a
# 421 ms baseline read 775 ms -- a 1.8x inflation that lands wherever the
# scheduler puts it, and leaves a plausible-looking average behind. A frame
# distribution is MORE sensitive to this than a total, because contention
# shows up as tail latency, which is precisely what the 60fps bar reads.
LOAD=$(uptime | sed 's/.*load averages*: *//' | awk '{print $1}' | tr -d ,)
if [[ -n "$LOAD" ]] && (( $(printf '%s > %s\n' "$LOAD" "$MAX_LOAD" | bc -l) )); then
    print -u2 "REFUSING TO MEASURE: 1-minute load is $LOAD, above --max-load $MAX_LOAD."
    print -u2 "  A contended machine inflated a 421 ms baseline to 775 ms (perf-method rule 4)."
    print -u2 "  Tail latency is the statistic the 60fps bar reads, and it is the one"
    print -u2 "  contention distorts most. Wait for the machine, or pass --max-load."
    pgrep -l rustc | head -5 >&2
    exit 3
fi

# --- Machine-local ROM/capture paths. Gitignored: no game content in-repo.
if [[ -f .claude/local.env ]]; then
    source .claude/local.env
else
    print -u2 "missing .claude/local.env (machine-local ROM paths)"; exit 1
fi

export ROM="${ROM:-$FN64_DISCOVER_NWXE_ROM}"
if [[ ! -f "$ROM" ]]; then
    print -u2 "ROM not found: $ROM"; exit 1
fi

# The three general-exception executable images and the black-box boot context
# handoff. Without these the route does not reach rendering at all.
C="${FN64_CAPTURES_DIR:-$HOME/Code/aki-recomp/captures}"
G="$C/wm-general-exception-images"
export FN64_EXECUTABLE_IMAGES="$G/run-1/image.json:$G/run-2/image.json:$G/run-3/image.json"
export FN64_BOOT_CONTEXT="${FN64_BOOT_CONTEXT:-$C/wm2000-boot-context.json}"
for required in "$G/run-1/image.json" "$FN64_BOOT_CONTEXT"; do
    if [[ ! -f "$required" ]]; then
        print -u2 "missing required capture: $required"
        print -u2 "  set FN64_CAPTURES_DIR if your captures live elsewhere"
        exit 1
    fi
done

export FN64_ABSENT_N64DD=1
export FN64_BLOCK_CONTINUE_AFTER_OVERLAY=1
# The write barrier is the committed configuration, not an A/B lane: leaving it
# off measures a program nobody ships.
export FN64_MPROTECT_BARRIER=1
export FN64_CONTROLLER_SCHEDULE="$REPO/reference/wm2000-routes/entrance-to-match.schedule"
export FN64_BLOCK_MAX_STEPS="$STEPS"
# Rule 7: print on a cadence the PROCESS controls. The harness otherwise logs
# only on a controller-input EDGE, and this schedule has no edge after read
# 600 -- so a healthy run and a wedged one emit byte-identical stdout.
export FN64_HEARTBEAT="$HEARTBEAT"

export FN64_FRAME_CENSUS=1
export FN64_FRAME_CENSUS_WARMUP_GFX="$WARMUP_GFX"

if [[ -n "$DUMP_DIR" ]]; then
    mkdir -p "$DUMP_DIR"
    export FN64_RENDER_DUMP_DIR="$DUMP_DIR"
    export FN64_RENDER_DUMP_LIMIT=4000
fi

if [[ -z "$BINARY" ]]; then
    BINARY="$REPO/examples/wm2000-block-boot/target/release/wm2000-block-boot"
fi
if [[ ! -x "$BINARY" ]]; then
    print -u2 "benchmark binary not found: $BINARY"
    print -u2 ""
    print -u2 "  Build it with THIS SCRIPT'S ENVIRONMENT ALREADY EXPORTED. The build"
    print -u2 "  script itself reads ROM, FN64_EXECUTABLE_IMAGES and FN64_BOOT_CONTEXT"
    print -u2 "  to discover and emit the shard catalog -- a plain \`cargo build\` with"
    print -u2 "  only ROM set fails in build.rs, not at run time:"
    print -u2 ""
    print -u2 "    source .claude/local.env"
    print -u2 "    export ROM=\"\$FN64_DISCOVER_NWXE_ROM\""
    print -u2 "    C=\$HOME/Code/aki-recomp/captures; G=\"\$C/wm-general-exception-images\""
    print -u2 "    export FN64_EXECUTABLE_IMAGES=\"\$G/run-1/image.json:\$G/run-2/image.json:\$G/run-3/image.json\""
    print -u2 "    export FN64_BOOT_CONTEXT=\"\$C/wm2000-boot-context.json\" FN64_ABSENT_N64DD=1"
    print -u2 "    cd examples/wm2000-block-boot && cargo build --release --bin wm2000-block-boot"
    print -u2 ""
    print -u2 "  If this tree's shard build is broken by another agent's in-flight work,"
    print -u2 "  build in a throwaway worktree off committed HEAD and pass --binary."
    print -u2 "  Note .claude/local.env is gitignored, so copy it into the worktree."
    exit 1
fi

print "route:        entrance-to-match.schedule"
print "binary:       $BINARY"
print "steps:        $STEPS"
print "warmup_gfx:   $WARMUP_GFX  (fields before this many graphics submits are the transient)"
print "load:         $LOAD"
print ""

# The UNFILTERED stream, at a per-run path. Load-bearing, and it cost a
# completed full-route census run to learn (perf-method rule 27).
#
# The filter below is an allowlist, so ANY report tag not named in it is
# invisible in whatever log the caller captured -- that silently ate both a new
# `[mirror-reconcile]` census and the pre-existing `[mprotect-barrier]` stats.
# The old destination was a FIXED `/tmp` path that every subsequent run
# overwrote, so by the time the omission was noticed the evidence was already
# destroyed, and the instrument looked broken when it was working perfectly.
#
# A unique path per run means a dropped tag is recoverable after the fact
# instead of needing the whole run repeated.
FULL_LOG="${FN64_BENCHMARK_FULL_LOG:-/tmp/fn64-render-benchmark-$$-$(date +%Y%m%d-%H%M%S).log}"

# `--line-buffered` is load-bearing: this run takes minutes, and a block-
# buffered filter shows nothing until the process exits, which is
# indistinguishable from a wedge. The heartbeat exists precisely so progress is
# observable while running (rule 7); swallowing it in a pipe buffer defeats it.
#
# `[mprotect-barrier]` and `[mirror-reconcile]` are in the allowlist because
# they are gated diagnostics that print only when explicitly armed -- when off
# they cost nothing, and when on they are the reason the run was made.
#
# `[frame-populations]`, `[executor-split]` and `[resume-split]` are here
# because the NOT-ARMED warnings ride on them. That is rule 27's sharpening:
# a warning on a filtered channel is not a warning, and two 25-minute runs were
# lost to an `[executor-split]` "NOT ARMED" notice that was printed, filtered,
# and never seen. Read the unfiltered log regardless -- the allowlist is a
# convenience for watching a live run, never the evidence.
"$BINARY" 2>&1 | tee "$FULL_LOG" | grep -E --line-buffered \
    '^\[frame-census\]|^\[fn64-heartbeat\]|render_error|steady idle|^\[wm2000-block-boot\] done|^\[mprotect-barrier\]|^\[mirror-reconcile\]|^\[frame-populations\]|^\[executor-split\]|^\[resume-split\]|^\[frame-sequence\] pattern|vi_reachability'

# The census prints from `atexit`, so it lands after the harness's own summary.
print ""
print "full log (UNFILTERED, per-run): $FULL_LOG"
print ""
print "READING THE RESULT"
print "  RATIO A is the 60fps bar (target 16.667 ms/field). RATIO B is speed"
print "  versus the console (target 1.000x). Report BOTH -- they answer"
print "  different questions and quoting one for the other is a known error."
print "  This is the HEADLESS lane: guest+runtime only, no presentation cost."
