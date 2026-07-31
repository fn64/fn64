#!/bin/zsh

# Run one declared WM2000 input scenario sequentially under the memory guard,
# validate its pure-static milestones, and require byte-identical semantic
# evidence. Existing generated AOT artifacts are reused; this does not build.

set -eu

if (( $# < 2 || $# > 4 )); then
    print -u2 -- "usage: ROM=... FN64_BOOT_CONTEXT=... $0 SCHEDULE MAX_STEPS [RUNS] [REQUIRED_GENERATIONS]"
    exit 2
fi

typeset -r gate_root=${0:A:h:h}
typeset -r gate_schedule=${1:A}
typeset -r gate_max_steps=$2
typeset -r gate_runs=${3:-10}
typeset -r gate_required_generations=${4:-${FN64_SCENARIO_REQUIRED_GENERATIONS:-}}
typeset -r gate_binary=${FN64_WM_PROBE_BINARY:-$gate_root/examples/wm2000-block-boot/target/debug/wm2000-block-boot}
typeset -r gate_min_input_edges=${FN64_SCENARIO_MIN_INPUT_EDGES:-1}
typeset -r gate_min_controller_ops=${FN64_SCENARIO_MIN_CONTROLLER_OPS:-1}
typeset -r gate_min_standard_reads=${FN64_SCENARIO_MIN_STANDARD_READS:-1}
typeset -r gate_min_gfx=${FN64_SCENARIO_MIN_GFX:-1}
typeset -r gate_min_audio=${FN64_SCENARIO_MIN_AUDIO:-0}
typeset -r gate_min_rcp_completed=${FN64_SCENARIO_MIN_RCP_COMPLETED:-1}
typeset -r gate_min_ucode=${FN64_SCENARIO_MIN_UCODE:-1}
typeset -r gate_min_dpc_commits=${FN64_SCENARIO_MIN_DPC_COMMITS:-1}

if [[ ! "$gate_runs" =~ '^[1-9][0-9]*$' ]] || (( gate_runs < 10 || gate_runs > 100 )); then
    print -u2 -- "wm2000 scenario gate: RUNS must be a canonical integer in 10..100"
    exit 2
fi
if [[ ! "$gate_max_steps" =~ '^[1-9][0-9]*$' ]]; then
    print -u2 -- "wm2000 scenario gate: MAX_STEPS must be a canonical positive integer"
    exit 2
fi
for minimum in \
    "$gate_min_input_edges" \
    "$gate_min_controller_ops" \
    "$gate_min_standard_reads" \
    "$gate_min_gfx" \
    "$gate_min_audio" \
    "$gate_min_rcp_completed" \
    "$gate_min_ucode" \
    "$gate_min_dpc_commits"; do
    if [[ ! "$minimum" =~ '^(0|[1-9][0-9]*)$' ]]; then
        print -u2 -- "wm2000 scenario gate: FN64_SCENARIO_MIN_* values must be nonnegative integers"
        exit 2
    fi
done
if [[ -n "$gate_required_generations" ]]; then
    if [[ ! "$gate_required_generations" =~ '^(0|[1-9][0-9]*)(,(0|[1-9][0-9]*))*$' ]]; then
        print -u2 -- "wm2000 scenario gate: REQUIRED_GENERATIONS must be canonical comma-separated decimal u64 values"
        exit 2
    fi
    typeset -A seen_generations
    for generation_id in ${(s:,:)gate_required_generations}; do
        if (( ${#generation_id} > 20 )) || \
            { (( ${#generation_id} == 20 )) && [[ "$generation_id" > 18446744073709551615 ]]; }; then
            print -u2 -- "wm2000 scenario gate: required generation is outside u64"
            exit 2
        fi
        if [[ -n ${seen_generations[$generation_id]:-} ]]; then
            print -u2 -- "wm2000 scenario gate: required generations must not contain duplicates"
            exit 2
        fi
        seen_generations[$generation_id]=1
    done
fi
if [[ -z ${ROM:-} || ! -f "$ROM" ]]; then
    print -u2 -- "wm2000 scenario gate: ROM must name the user's readable ROM"
    exit 2
fi
if [[ -z ${FN64_BOOT_CONTEXT:-} || ! -f "$FN64_BOOT_CONTEXT" ]]; then
    print -u2 -- "wm2000 scenario gate: FN64_BOOT_CONTEXT must name a readable capture"
    exit 2
fi
if [[ ! -f "$gate_schedule" ]]; then
    print -u2 -- "wm2000 scenario gate: schedule does not exist"
    exit 2
fi
if [[ ! -x "$gate_binary" ]]; then
    print -u2 -- "wm2000 scenario gate: selected probe binary is not executable"
    exit 2
fi

hash_file() {
    shasum -a 256 -- "$1" | awk '{print $1}'
}

typeset -r gate_rom_sha256=$(hash_file "$ROM")
typeset -r gate_boot_context_sha256=$(hash_file "$FN64_BOOT_CONTEXT")
typeset -r gate_schedule_sha256=$(hash_file "$gate_schedule")
typeset -r gate_binary_sha256=$(hash_file "$gate_binary")
typeset -r gate_policy="scenario policy schema=1 runs=$gate_runs max_steps=$gate_max_steps required_generations=${gate_required_generations:-none} min_input_edges=$gate_min_input_edges min_controller_ops=$gate_min_controller_ops min_standard_reads=$gate_min_standard_reads min_gfx=$gate_min_gfx min_audio=$gate_min_audio min_rcp_completed=$gate_min_rcp_completed min_ucode=$gate_min_ucode min_dpc_commits=$gate_min_dpc_commits rom_sha256=$gate_rom_sha256 boot_context_sha256=$gate_boot_context_sha256 schedule_sha256=$gate_schedule_sha256 binary_sha256=$gate_binary_sha256"
typeset -r gate_logs=$(mktemp -d /private/tmp/fn64-wm-scenario-gate.XXXXXX)
typeset -r gate_baseline=$gate_logs/baseline.evidence

if ! "$gate_root/scripts/check-wm2000-pure-aot.zsh"; then
    print -u2 -- "wm2000 scenario gate: production feature graph rejected"
    exit 1
fi
export FN64_PHASE_TIMING=1

for run_index in {1..$gate_runs}; do
    typeset run_log=$gate_logs/run-$run_index.log
    typeset run_evidence=$gate_logs/run-$run_index.evidence
    print -u2 -- "wm2000 scenario gate: run $run_index/$gate_runs"
    if ! "$gate_root/scripts/wm2000-route-probe.zsh" \
        "$gate_schedule" "$gate_max_steps" >"$run_log" 2>&1; then
        print -u2 -- "wm2000 scenario gate: run $run_index failed; logs retained at $gate_logs"
        exit 1
    fi
    print -r -- "$gate_policy" >"$run_evidence"
    if ! awk \
        -v required_generations="$gate_required_generations" \
        -v min_input_edges="$gate_min_input_edges" \
        -v min_controller_ops="$gate_min_controller_ops" \
        -v min_standard_reads="$gate_min_standard_reads" \
        -v min_gfx="$gate_min_gfx" \
        -v min_audio="$gate_min_audio" \
        -v min_rcp_completed="$gate_min_rcp_completed" \
        -v min_ucode="$gate_min_ucode" \
        -v min_dpc_commits="$gate_min_dpc_commits" \
        -f "$gate_root/scripts/check-wm2000-scenario-log.awk" \
        "$run_log" >>"$run_evidence"; then
        print -u2 -- "wm2000 scenario gate: run $run_index missed a required milestone; logs retained at $gate_logs"
        exit 1
    fi
    if (( run_index == 1 )); then
        cp "$run_evidence" "$gate_baseline"
    elif ! cmp -s "$gate_baseline" "$run_evidence"; then
        print -u2 -- "wm2000 scenario gate: run $run_index differs; logs retained at $gate_logs"
        diff -u "$gate_baseline" "$run_evidence" || true
        exit 1
    fi
    shasum -a 256 "$run_evidence"
done

print -- "wm2000 scenario gate: $gate_runs/$gate_runs valid and evidence-identical; logs=$gate_logs"
