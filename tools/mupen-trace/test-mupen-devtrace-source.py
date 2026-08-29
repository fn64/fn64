#!/usr/bin/env python3
"""Source-level contract checks for the device timing producer."""

from pathlib import Path


SOURCE = Path(__file__).with_name("mupen_devtrace.c").read_text(encoding="utf-8")
WIRE = Path(__file__).with_name("mupen_devtrace_wire.h").read_text(encoding="utf-8")


def main() -> None:
    required_source = (
        '#include "mupen_devtrace_wire.h"',
        '"mupen-devtrace v3 (mupen64plus-core DEBUGGER=1 pure-interpreter + rsp plugin, "',
        "fn64_count_clock_init(&count_clock, cop0[9]);",
        "fn64_count_clock_observe(&count_clock, count_now, &cycle)",
        "fn64_instruction_writes_cp0_count(DebugMemRead32(pc))",
        'getenv("FN64_DEVICE_TRACE_SCOPE")',
        "(timing_scope & FN64_SCOPE_PI) != 0",
        "(timing_scope & FN64_SCOPE_VI) != 0",
        "fn64_classify_pi_observation(cart_addr, dram_addr, rd_len, wr_len,",
        "uint32_t mi_now = DebugMemRead32(MI_INTR);",
        "if (!fn64_pi_completion_is_proven(mi_prev, mi_now))",
        'fn64_emit_timing_pi_event(out, ordinal++, "dma_start",',
        'fn64_emit_timing_pi_event(out, ordinal++, "dma_complete",',
        "fn64_event_clock_stamp(&event_clock, cycle)",
        "static _Atomic uint64_t g_vi_callbacks;",
        "atomic_fetch_add_explicit(&g_vi_callbacks, UINT64_C(1), memory_order_relaxed);",
        "atomic_load_explicit(&g_vi_callbacks, memory_order_relaxed);",
        "vi_callbacks_now - vi_callbacks_prev > UINT64_C(1)",
        'fn64_emit_timing_event(out, ordinal++, "vi_retrace", "vi",',
        "refusing to fabricate VI timing",
        'fn64_emit_timing_end(out, ordinal, "aborted")',
    )
    required_wire = (
        '"schema_version\\\":3',
        '"unit\\\":\\\"r4300_master_cycle',
        '"origin\\\":\\\"first_event',
        '"quantum\\\":2',
        '"observed_devices\\\":[',
        "fn64_parse_timing_scope",
        '"dma_direction\\\":null',
        '"pi_device\\\":null',
        '"pi_offset\\\":null',
        'return direction == FN64_PI_TO_RDRAM ? "to_rdram" : "from_rdram";',
        'return device == FN64_PI_ROM ? "rom" : "sram";',
        "FN64_PI_DOM1_A2_START",
        "FN64_PI_DOM2_A2_START",
        "fn64_pi_completion_is_proven",
    )
    missing = [fragment for fragment in required_source if fragment not in SOURCE]
    missing += [fragment for fragment in required_wire if fragment not in WIRE]
    if missing:
        raise SystemExit("mupen devtrace v3 source contract missing: " + ", ".join(missing))

    classify = SOURCE.index("fn64_classify_pi_observation(cart_addr")
    reject = SOURCE.index("if (observation_error != FN64_PI_OBSERVATION_OK)", classify)
    start = SOURCE.index('fn64_emit_timing_pi_event(out, ordinal++, "dma_start"', reject)
    falling = SOURCE.index("} else if (!pi_busy && pi_prev.busy)", start)
    proof = SOURCE.index("if (!fn64_pi_completion_is_proven(mi_prev, mi_now))", falling)
    complete = SOURCE.index('fn64_emit_timing_pi_event(out, ordinal++, "dma_complete"', start)
    if not classify < reject < start < complete:
        raise SystemExit("PI start must be classified and rejected before v3 emission")
    raised = SOURCE.index("uint32_t raised = mi_now & ~mi_prev;", complete)
    mi_raise = SOURCE.index('"mi_raise"', raised)
    if not falling < proof < complete < raised < mi_raise:
        raise SystemExit(
            "PI BUSY fall must be proven by the same MI sample before completion, "
            "then emit completion before the matching MI raise"
        )
    if '"schema_version\\\":1' in WIRE or '"schema_version\\\":2' in WIRE:
        raise SystemExit("timing wire helper still contains a pre-v3 schema")
    vi_emit = SOURCE.index('fn64_emit_timing_event(out, ordinal++, "vi_retrace", "vi",')
    mi_diff = SOURCE.index("uint32_t raised = mi_now & ~mi_prev;", vi_emit)
    if not vi_emit < mi_diff:
        raise SystemExit("callback-derived VI must precede MI edges from the same pause")
    if "DebugMemRead32(VI_CURRENT)" in SOURCE:
        raise SystemExit("timing producer must not infer VI from VI_CURRENT polling")
    print("mupen-devtrace v3 source contract: ok")


if __name__ == "__main__":
    main()
