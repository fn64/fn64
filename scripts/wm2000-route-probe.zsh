#!/bin/zsh

# Run one deterministic WM2000 input route against the existing dense-AOT
# binary with constant-space diagnostics and the repository memory guard.

set -u

if (( $# < 1 || $# > 3 )); then
    print -u2 -- "usage: ROM=... FN64_BOOT_CONTEXT=... $0 SCHEDULE [MAX_STEPS] [STOP_GENERATION]"
    exit 2
fi
if [[ -z "${ROM:-}" || ! -f "$ROM" ]]; then
    print -u2 -- "wm2000 route probe: ROM must name the user's readable ROM"
    exit 2
fi
if [[ -z "${FN64_BOOT_CONTEXT:-}" || ! -f "$FN64_BOOT_CONTEXT" ]]; then
    print -u2 -- "wm2000 route probe: FN64_BOOT_CONTEXT must name a readable capture"
    exit 2
fi

typeset -r probe_root=${0:A:h:h}
typeset -r probe_schedule=${1:A}
typeset -r probe_max_steps=${2:-250000}
# Release by default: this route runs hundreds of thousands of steps, and the
# debug binary makes that a multi-hour wait. Override with FN64_WM_PROBE_BINARY.
typeset -r probe_binary=${FN64_WM_PROBE_BINARY:-$probe_root/examples/wm2000-block-boot/target/release/wm2000-block-boot}
typeset -r probe_guard_max_rss_mib=${FN64_GUARD_MAX_RSS_MIB:-2048}
typeset -a probe_environment

if [[ ! -f "$probe_schedule" ]]; then
    print -u2 -- "wm2000 route probe: schedule does not exist: $probe_schedule"
    exit 2
fi
if [[ ! "$probe_max_steps" =~ '^[1-9][0-9]*$' ]]; then
    print -u2 -- "wm2000 route probe: MAX_STEPS must be a positive integer"
    exit 2
fi
if [[ ! -x "$probe_binary" ]]; then
    print -u2 -- "wm2000 route probe: build the harness first; executable not found: $probe_binary"
    exit 2
fi

# This wrapper promises a constant-space route probe. Stale opt-in histories
# from a prior diagnostic shell would otherwise defeat progress-only mode.
unset FN64_BLOCK_EXECUTOR_TRACE
unset FN64_BLOCK_DEVICE_TRACE
unset FN64_BLOCK_PC_TRACE
unset FN64_BLOCK_HOST_TRACE
export FN64_GUARD_MAX_RSS_MIB=$probe_guard_max_rss_mib

probe_environment=(
    "ROM=$ROM"
    "FN64_BOOT_CONTEXT=$FN64_BOOT_CONTEXT"
    "FN64_CONTROLLER_SCHEDULE=$probe_schedule"
    "FN64_BLOCK_MAX_STEPS=$probe_max_steps"
    FN64_BLOCK_CONTINUE_AFTER_OVERLAY=1
    FN64_BLOCK_PROGRESS_ONLY=1
)
# The allowlist is deliberate -- it is what makes this wrapper reproducible --
# but it silently dropped two variables the route cannot run without.
#
# FN64_ABSENT_N64DD: WM2000's IPL probes the N64DD window at 0xa6000000. On a
# cartridge-only ROM that read has no modelled device, and the unwinding panic
# at pi/timing.rs:80 aborts the route before any gameplay. Opting in is how
# every other lane runs this ROM.
#
# FN64_EXECUTABLE_IMAGES: without the captured exception images the harness
# cannot admit the general-exception preamble.
#
# Forwarded only when set, so the wrapper stays honest about its inputs.
if [[ -n "$FN64_ABSENT_N64DD" ]]; then
    probe_environment+=("FN64_ABSENT_N64DD=$FN64_ABSENT_N64DD")
fi
if [[ -n "$FN64_EXECUTABLE_IMAGES" ]]; then
    probe_environment+=("FN64_EXECUTABLE_IMAGES=$FN64_EXECUTABLE_IMAGES")
fi
if (( $# == 3 )); then
    probe_environment+=("FN64_PROFILE_STOP_AT_GENERATION=$3")
fi

exec "$probe_root/scripts/memory-guard.zsh" /usr/bin/env "${probe_environment[@]}" "$probe_binary"
