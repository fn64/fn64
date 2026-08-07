#!/bin/zsh

# A self-time profile of wm2000-block-boot that refuses to lie.
#
# `docs/plans/resolvable-self-time-profile.md` records two confidently WRONG
# profiles produced by this exact toolchain, plus three optimizations dispatched
# off bad numbers. Every guard below exists because one of those happened:
#
#   1. LOAD GATE. A sampling profiler on a contended machine attributes time to
#      whatever gets descheduled. Measured 2026-08-07: with 15 competing rustc
#      processes at load 20, the same route ran 775 ms against the 420-440 ms it
#      records at load ~1 -- a 1.8x inflation that lands wherever the scheduler
#      puts it. This script refuses to run above --max-load and says why.
#   2. PER-RUN ASLR SLIDE. Runs are individually slid, so each trace must be
#      converted with its OWN `load-addr` from the export. An inferred or
#      shared slide produced a top-15 list led by `PathBuf::__set_extension`
#      in a loop that touches no paths.
#   3. SELF TIME, NOT INCLUSIVE. Self time is the LEAF frame only, weighted by
#      the row's cycle-weight. Reading inclusive totals as self time caused
#      three consecutive failed optimizations.
#   4. dSYM REQUIRED. Without line tables `run_one_step` inlines its whole
#      callee tree and ~92% of self time lands in one unbreakable frame. The
#      `debug = 1` profile overrides in the example's Cargo.toml are load-
#      bearing; this script verifies the dSYM actually resolved something.
#
# Usage:
#   scripts/profile-wm2000-self-time.zsh                 # 5 runs, deep route
#   scripts/profile-wm2000-self-time.zsh --runs 3
#   scripts/profile-wm2000-self-time.zsh --max-load 4    # relax the gate
#   scripts/profile-wm2000-self-time.zsh --wait          # block until quiet
#   scripts/profile-wm2000-self-time.zsh --selftest

set -eu
set -o pipefail

typeset -r repo_root=${0:A:h:h}
typeset -r script_path=$0
typeset runs=5
typeset max_load=2.5
typeset wait_for_quiet=0
typeset mode=run
typeset outdir=""

usage() {
    print -u2 -- "usage: $script_path [--runs N] [--max-load L] [--wait] [--out DIR]"
    print -u2 -- "       $script_path --selftest"
}

while (( $# )); do
    case $1 in
        --runs) runs=${2:?}; shift 2 ;;
        --max-load) max_load=${2:?}; shift 2 ;;
        --wait) wait_for_quiet=1; shift ;;
        --out) outdir=${2:?}; shift 2 ;;
        --selftest) mode=selftest; shift ;;
        -h|--help) usage; exit 0 ;;
        *) print -u2 -- "unknown argument: $1"; usage; exit 2 ;;
    esac
done

# --- the load gate -----------------------------------------------------------

current_load() {
    uptime | sed -E 's/.*load averages?: ([0-9.]+).*/\1/'
}

# Processes that specifically invalidate a profile of THIS binary: compilers and
# other copies of the workload. Counted for the diagnostic, not just the gate.
contending_processes() {
    ps -A -o comm | grep -cE 'bin/(rustc|cc1|clang)$|wm2000-block-boot$' || true
}

assert_quiet_enough() {
    local load contenders
    load=$(current_load)
    contenders=$(contending_processes)
    if (( $(print -- "$load > $max_load" | bc -l) )); then
        print -u2 -- "REFUSING TO PROFILE: load average $load exceeds --max-load $max_load"
        print -u2 -- "  competing rustc/clang/wm2000 processes: $contenders"
        print -u2 -- ""
        print -u2 -- "  A sampling profiler under contention attributes time to whatever gets"
        print -u2 -- "  descheduled, not to what is slow. docs/plans/resolvable-self-time-profile.md"
        print -u2 -- "  records two confidently-wrong profiles from exactly this setup."
        print -u2 -- ""
        print -u2 -- "  Wait for the machine to quiesce, re-run with --wait, or raise --max-load"
        print -u2 -- "  deliberately if you accept the noise."
        return 1
    fi
    print -- "[gate] load=$load contenders=$contenders (limit $max_load) -- ok"
    return 0
}

wait_until_quiet() {
    local waited=0
    while ! assert_quiet_enough 2>/dev/null; do
        if (( waited == 0 )); then
            print -- "[gate] machine busy (load $(current_load), $(contending_processes) contenders); waiting..."
        fi
        sleep 15
        (( waited += 15 ))
        if (( waited % 120 == 0 )); then
            print -- "[gate] still waiting after ${waited}s (load $(current_load))"
        fi
    done
    (( waited > 0 )) && print -- "[gate] quiet after ${waited}s"
    return 0
}

# --- selftest ----------------------------------------------------------------
#
# Proves the gate actually gates. A profiling script whose safety check is
# untested is the same failure this script exists to prevent.

if [[ $mode == selftest ]]; then
    typeset failures=0
    check() {
        if eval "$2"; then print -- "  ok   $1"; else print -- "  FAIL $1"; (( failures++ )); fi
    }
    print -- "selftest: the load gate"

    # `max_load` is a script-scoped typeset, so a `VAR=x func` prefix does NOT
    # scope to it the way it would for an external command -- set it explicitly
    # and restore, which is also how the real invocation reaches the gate.
    typeset saved_max_load=$max_load

    # A max-load of 0 must always refuse: no machine has load < 0.
    max_load=0
    check "refuses at max-load 0" '! assert_quiet_enough >/dev/null 2>&1'
    # The refusal must explain itself on stderr, not fail silently. Capture
    # stderr only: 2>&1 >/dev/null reorders wrongly here, so use a temp file.
    typeset refusal_log=$(mktemp)
    assert_quiet_enough >/dev/null 2>"$refusal_log" || true
    check "refusal mentions the load average" 'grep -q "REFUSING TO PROFILE" "$refusal_log"'
    check "refusal reports the actual load number" \
        'grep -qE "load average [0-9]+\.[0-9]+" "$refusal_log"'
    check "refusal cites the wrong-profile precedent" \
        'grep -q "resolvable-self-time-profile" "$refusal_log"'
    check "refusal names the escape hatches" \
        'grep -q -- "--wait" "$refusal_log" && grep -q -- "--max-load" "$refusal_log"'
    rm -f "$refusal_log"

    # A max-load of 100000 must always pass.
    max_load=100000
    check "accepts at max-load 100000" 'assert_quiet_enough >/dev/null 2>&1'
    max_load=$saved_max_load
    # current_load must yield a number.
    check "current_load parses as a number" \
        '( print -- "$(current_load)" | grep -qE "^[0-9]+\.[0-9]+$" )'
    check "contending_processes yields an integer" \
        '( print -- "$(contending_processes)" | grep -qE "^[0-9]+$" )'

    print -- ""
    if (( failures )); then print -u2 -- "selftest: $failures failure(s)"; exit 1; fi
    print -- "selftest: all checks passed"
    exit 0
fi

# --- environment -------------------------------------------------------------

if [[ ! -f "$repo_root/.claude/local.env" ]]; then
    print -u2 -- "missing $repo_root/.claude/local.env (machine-local ROM paths)"
    exit 1
fi
source "$repo_root/.claude/local.env"

export ROM="${FN64_DISCOVER_NWXE_ROM:?FN64_DISCOVER_NWXE_ROM unset}"
typeset -r captures="$HOME/Code/aki-recomp/captures"
typeset -r images="$captures/wm-general-exception-images"
export FN64_EXECUTABLE_IMAGES="$images/run-1/image.json:$images/run-2/image.json:$images/run-3/image.json"
export FN64_BOOT_CONTEXT="$captures/wm2000-boot-context.json"
# The deep route. FN64_EXECUTABLE_IMAGES and FN64_BOOT_CONTEXT are needed at
# BUILD time as well as run time -- build.rs asserts on the former -- which is
# why a binary built without them silently profiles a different program.
export FN64_ABSENT_N64DD=1 FN64_BLOCK_MAX_STEPS=19523 FN64_MPROTECT_BARRIER=1

typeset -r binary="$repo_root/examples/wm2000-block-boot/target/release/wm2000-block-boot"
if [[ ! -x $binary ]]; then
    print -u2 -- "missing $binary -- build it first:"
    print -u2 -- "  cd examples/wm2000-block-boot && cargo build --release --bin wm2000-block-boot"
    exit 1
fi

[[ -n $outdir ]] || outdir=$(mktemp -d /tmp/wm-profile.XXXXXX)
mkdir -p "$outdir"
print -- "[out] $outdir"

# --- the gate, for real ------------------------------------------------------

if (( wait_for_quiet )); then
    wait_until_quiet
else
    assert_quiet_enough || exit 1
fi

# --- dSYM --------------------------------------------------------------------

typeset -r dsym="$outdir/wm2000bb.dSYM"
print -- "[dsym] generating..."
dsymutil "$binary" -o "$dsym"

# A dSYM that resolves nothing yields a profile of one giant inlined frame.
# Prove it resolves a symbol we know is there before spending five runs on it.
if ! atos -o "$dsym" -l 0x100000000 0x100000000 >/dev/null 2>&1; then
    print -u2 -- "dSYM at $dsym does not resolve -- the profile would be one inlined frame."
    print -u2 -- "Check the [profile.release.package.*] debug=1 overrides in"
    print -u2 -- "examples/wm2000-block-boot/Cargo.toml."
    exit 1
fi

# --- record ------------------------------------------------------------------

typeset -a traces
for run in $(seq 1 $runs); do
    # Re-check between runs: a build can start halfway through a profile and
    # poison only the later traces, which is worse than poisoning all of them
    # because the average still looks plausible.
    if ! assert_quiet_enough >/dev/null 2>&1; then
        print -u2 -- "ABORTING at run $run: machine became busy mid-profile (load $(current_load))."
        print -u2 -- "Partial traces in $outdir are NOT a valid profile -- discard them."
        exit 1
    fi
    typeset trace="$outdir/run-$run.trace"
    print -- "[record] run $run/$runs"
    xctrace record --template "CPU Profiler" --output "$trace" \
        --target-stdout /dev/null --launch -- "$binary" >/dev/null 2>&1
    xctrace export --input "$trace" \
        --xpath '/trace-toc/run[@number="1"]/data/table[@schema="cpu-profile"]' \
        --output "$outdir/run-$run.xml" >/dev/null 2>&1
    traces+=("$outdir/run-$run.xml")
done

# --- resolve and aggregate ---------------------------------------------------
#
# Self time is the LEAF frame only, weighted by cycle-weight. Each run is slid
# independently, so each XML is converted with its own load-addr.

print -- "[resolve] aggregating $runs run(s)"
python3 "$repo_root/scripts/wm2000_self_time.py" --dsym "$dsym" "${traces[@]}"
