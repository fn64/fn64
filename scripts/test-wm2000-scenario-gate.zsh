#!/bin/zsh

# Fast, ROM-free parser regressions for the bounded WM2000 scenario gate.

set -eu

typeset -r test_root=${0:A:h:h}
typeset -r test_dir=$(mktemp -d /private/tmp/fn64-wm-scenario-parser.XXXXXX)
typeset -r valid_log=$test_dir/valid.log
typeset -r baseline=$test_dir/baseline.evidence

trap 'rm -rf -- "$test_dir"' EXIT

cat >"$valid_log" <<'EOF'
[wm2000-block-boot] static execution build schema=1 aot_runtime=true production_aot=true dev_interpreter=false
[wm2000-block-boot] canonical program artifact=0101010101010101010101010101010101010101010101010101010101010101
[wm2000-block-boot] controller schedule=/tmp/route phases=1 sha256=abababababababababababababababababababababababababababababababab
[wm2000-block-boot] controller input_edge port=0 read=1 buttons=0x1 stick=(0, 0) step=1 sim_time=2 gfx_submits=1 audio_submits=1 generations=[123]
[wm2000-block-boot] done: steps=2 sim_time=3 thread0_dead=false
[wm2000-block-boot] standard controller reads port0=1 port1=0 port2=0 port3=0
[wm2000-block-progress] trace=1 device_trace=1 gfx_submits=1 audio_submits=1 rcp_completed=1 controller_ops=1 ucode_recognitions=1 dram_dpc=0 xbus_dpc=1 render_error=None
[wm2000-block-profile] phase_timing executor_ms=1.0 calls=2
[wm2000-block-boot] entered digest-selected ROM-recovered generations: [123]
[wm2000-block-boot] bounded progress-only exit: ProcessExitSummary { threads: 1, detached_coroutines: 0 }
EOF

for run_index in {1..10}; do
    typeset evidence=$test_dir/run-$run_index.evidence
    awk -v required_generations=123 \
        -f "$test_root/scripts/check-wm2000-scenario-log.awk" \
        "$valid_log" >"$evidence"
    if (( run_index == 1 )); then
        cp "$evidence" "$baseline"
    else
        cmp -s "$baseline" "$evidence"
    fi
done

if awk -v required_generations=999 \
    -f "$test_root/scripts/check-wm2000-scenario-log.awk" \
    "$valid_log" >/dev/null 2>&1; then
    print -u2 -- "scenario parser accepted an absent required generation"
    exit 1
fi

if sed 's/gfx_submits=1/gfx_submits=0/g' "$valid_log" | \
    awk -v required_generations=123 \
        -f "$test_root/scripts/check-wm2000-scenario-log.awk" \
        >/dev/null 2>&1; then
    print -u2 -- "scenario parser accepted zero graphics submissions"
    exit 1
fi

for failure_marker in AotMiss MissingAotEntry ImageChanged UnknownBank UnsupportedInstruction; do
    if { cat "$valid_log"; print -- "$failure_marker"; } | \
        awk -v required_generations=123 \
            -f "$test_root/scripts/check-wm2000-scenario-log.awk" \
            >/dev/null 2>&1; then
        print -u2 -- "scenario parser accepted $failure_marker"
        exit 1
    fi
done

if sed 's/canonical program artifact=[0-9a-f]*/canonical program artifact=01/' "$valid_log" | \
    awk -v required_generations=123 \
        -f "$test_root/scripts/check-wm2000-scenario-log.awk" \
        >/dev/null 2>&1; then
    print -u2 -- "scenario parser accepted a truncated artifact identity"
    exit 1
fi

if "$test_root/scripts/wm2000-scenario-gate.zsh" /dev/null 1 1 >/dev/null 2>&1; then
    print -u2 -- "authoritative scenario gate accepted fewer than ten runs"
    exit 1
fi

print -- "wm2000 scenario parser: 10/10 identical positive runs; typed negative cases rejected"
