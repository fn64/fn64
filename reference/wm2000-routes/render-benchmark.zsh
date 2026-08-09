#!/bin/zsh

# WM2000 render benchmark: per-frame latency during SUSTAINED RENDERING.
#
#
# WHERE DOES THE TIME GO? RUN THIS:
#
#     ./reference/wm2000-routes/render-benchmark.zsh --profile
#
# That is the whole answer. One command, one authoritative report, tagged
# `[fn64-profile]`. You do not need to know which env gates exist, and you
# should not set them by hand -- `--profile` exports the single `FN64_PROFILE`
# flag and the binary composes the rest, each at a value its OWN parser
# accepts. The gates disagree about what "on" means: `FN64_EXECUTOR_SPLIT=0`
# ARMS its instrument while `FN64_FRAME_CENSUS_POPULATIONS=0` disarms its own.
#
# What `--profile` guarantees, and why each guarantee exists:
#
#   * EVERY ROW STATES BOTH DENOMINATORS -- its share of its parent AND its
#     ratio to the 16.667 ms budget. "20.9% of resume NET" reads as small;
#     the same row as "0.57x budget" does not, and three such rows summed to
#     1.29x the budget while every individual number was correct.
#   * THE ROWS ARE SUMMED FOR YOU, against the budget. Do not eye a list.
#   * A CHILD EXCEEDING ITS PARENT REFUSES TO PRINT its subtree. That check
#     alone catches three of the four instrument defects found in one evening.
#   * A PARTIAL PROFILE IS REFUSED, exit 70, naming the gate that did not arm.
#     A plausible subset gets believed; nothing does not.
#   * PER POPULATION AND PER PERCENTILE (p50/p95/p99), never a bare mean. A
#     mean has hidden the real distribution twice: bimodal, then trimodal.
#   * PROVENANCE IN THE REPORT -- binary, route, step count, and each gate's
#     state VERIFIED BY EFFECT rather than echoed.
#   * GUEST BYTE-IDENTITY is checked automatically on the 1.5M route.
#
# If it refuses, read the refusal: it names the missing gate and what was lost.
# A refusal is the tool working. Full method: `docs/plans/perf-method.md`.
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
PROFILE=0
BASELINE_MS=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --steps)      STEPS="$2"; shift 2 ;;
        --warmup-gfx) WARMUP_GFX="$2"; shift 2 ;;
        --max-load)   MAX_LOAD="$2"; shift 2 ;;
        --heartbeat)  HEARTBEAT="$2"; shift 2 ;;
        --binary)     BINARY="$2"; shift 2 ;;
        --dump-dir)   DUMP_DIR="$2"; shift 2 ;;
        --profile)     PROFILE=1; shift ;;
        --baseline-ms) BASELINE_MS="$2"; shift 2 ;;
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

# --- `--profile`: ONE gate that arms every decomposition channel.
#
# This script historically exported NONE of the five gates a decomposition
# needs, so getting one meant remembering all five names and each gate's own
# truthiness convention -- and they do not agree. `FN64_EXECUTOR_SPLIT=0` ARMS
# the instrument (it is `var_os().is_some()`) while `FN64_FRAME_CENSUS_
# POPULATIONS=0` disarms it (`env_flag`). Two conventions, opposite meanings,
# identical-looking values.
#
# So this exports the single flag and lets the binary compose the rest, each at
# a value its own parser accepts. Do not expand this into five exports here:
# that reintroduces exactly the per-gate knowledge the flag exists to remove.
if (( PROFILE )); then
    export FN64_PROFILE=1
    # Rule 17: the instrument's cost is MEASURED, never predicted -- a
    # predicted 0.029 ms/field once measured +1.62, wrong by 56x. Pass the
    # control lane's ms/field in with --baseline-ms and the report states the
    # perturbation, the correction factor, and the resolution floor below
    # which a row cannot be trusted. Without it the header says UNMEASURED
    # rather than implying zero.
    if [[ -n "$BASELINE_MS" ]]; then
        export FN64_PROFILE_BASELINE_MS="$BASELINE_MS"
    fi
fi

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
# `^\[fn64-profile\]` is in the allowlist because this filter is an ALLOWLIST:
# a tag not named here is invisible in the stream the operator watches, and
# that has already cost two 25-minute runs and one whole census. The profile
# report is the authoritative output of `--profile`; dropping it here would
# make the flag silently useless. Add the tag in the SAME change that
# introduces it, always.
"$BINARY" 2>&1 | tee "$FULL_LOG" | grep -E --line-buffered \
    '^\[frame-census\]|^\[fn64-heartbeat\]|render_error|steady idle|^\[wm2000-block-boot\] done|^\[mprotect-barrier\]|^\[mirror-reconcile\]|^\[frame-populations\]|^\[executor-split\]|^\[resume-split\]|^\[fn64-profile\]|^\[frame-sequence\] pattern|vi_reachability'
BENCH_STATUS=${pipestatus[1]}

# The census prints from `atexit`, so it lands after the harness's own summary.
print ""
print "full log (UNFILTERED, per-run): $FULL_LOG"
print ""
print "READING THE RESULT"
print "  RATIO A is the 60fps bar (target 16.667 ms/field). RATIO B is speed"
print "  versus the console (target 1.000x). Report BOTH -- they answer"
print "  different questions and quoting one for the other is a known error."
print "  This is the HEADLESS lane: guest+runtime only, no presentation cost."

# --- Guest byte-identity, run automatically rather than left as a step a tired
# person can skip. The emulated program must not change: a perf number from a
# run that emulated something else is not a perf number.
#
# Uses the anchored checker and the recorded per-ROUTE tuple. The route is part
# of the tuple -- checking a 1.5M run against the 2.1M expectation fails and
# burns the run. Never hand-roll the extraction: a `findall(...)[-1]` scan once
# compared a steady-state span count against a whole-run total and cost three
# 25-minute runs.
IDENTITY_EXPECT="$REPO/scripts/byte-identity-1p5M.txt"
if (( PROFILE )) && [[ "$STEPS" == 1500000 && -f "$IDENTITY_EXPECT" ]]; then
    print ""
    print "GUEST BYTE-IDENTITY"
    if python3 "$REPO/scripts/check-byte-identity.py" "$IDENTITY_EXPECT" "$FULL_LOG"; then
        print "  guest byte-identity holds: the emulated program is unchanged."
    else
        print -u2 "  GUEST BYTE-IDENTITY FAILED -- this run emulated a different program."
        print -u2 "  Its timings describe that other program and must not be reported."
        exit 4
    fi
fi

# Propagate the binary's own exit status. Under FN64_PROFILE the binary exits
# non-zero when a constituent gate failed to arm, and a refusal that the script
# swallows into a green exit is not a refusal.
if (( BENCH_STATUS != 0 )); then
    print -u2 ""
    print -u2 "BENCHMARK EXITED $BENCH_STATUS."
    if (( BENCH_STATUS == 70 )); then
        print -u2 "  Exit 70 is FN64_PROFILE refusing to print a partial profile:"
        print -u2 "  a constituent channel did not arm. Grep the unfiltered log for"
        print -u2 "  '[fn64-profile] MISSING' -- it names the gate and what was lost."
    fi
    exit "$BENCH_STATUS"
fi
