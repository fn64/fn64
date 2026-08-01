#!/usr/bin/env python3
"""Source-level contract checks for the device timing producer."""

from pathlib import Path


SOURCE = Path(__file__).with_name("mupen_devtrace.c").read_text(encoding="utf-8")
WIRE = Path(__file__).with_name("mupen_devtrace_wire.h").read_text(encoding="utf-8")


def main() -> None:
    required_source = (
        '#include "mupen_devtrace_wire.h"',
        '"mupen-devtrace v2 (mupen64plus-core DEBUGGER=1 pure-interpreter + rsp plugin, "',
        "fn64_classify_pi_observation(cart_addr, dram_addr, rd_len, wr_len,",
        "uint32_t mi_now = DebugMemRead32(MI_INTR);",
        "if (!fn64_pi_completion_is_proven(mi_prev, mi_now))",
        'fn64_emit_timing_pi_event(out, ordinal++, "dma_start", cycle,',
        'fn64_emit_timing_pi_event(out, ordinal++, "dma_complete", cycle,',
        'fn64_emit_timing_end(out, ordinal, "aborted")',
    )
    required_wire = (
        '"schema_version\\\":2',
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
        raise SystemExit("mupen devtrace v2 source contract missing: " + ", ".join(missing))

    classify = SOURCE.index("fn64_classify_pi_observation(cart_addr")
    reject = SOURCE.index("if (observation_error != FN64_PI_OBSERVATION_OK)", classify)
    start = SOURCE.index('fn64_emit_timing_pi_event(out, ordinal++, "dma_start"', reject)
    falling = SOURCE.index("} else if (!pi_busy && pi_prev.busy)", start)
    proof = SOURCE.index("if (!fn64_pi_completion_is_proven(mi_prev, mi_now))", falling)
    complete = SOURCE.index('fn64_emit_timing_pi_event(out, ordinal++, "dma_complete"', start)
    if not classify < reject < start < complete:
        raise SystemExit("PI start must be classified and rejected before v2 emission")
    raised = SOURCE.index("uint32_t raised = mi_now & ~mi_prev;", complete)
    mi_raise = SOURCE.index('"mi_raise"', raised)
    if not falling < proof < complete < raised < mi_raise:
        raise SystemExit(
            "PI BUSY fall must be proven by the same MI sample before completion, "
            "then emit completion before the matching MI raise"
        )
    if '"schema_version\\\":1' in WIRE:
        raise SystemExit("timing wire helper still contains schema v1")
    print("mupen-devtrace v2 source contract: ok")


if __name__ == "__main__":
    main()
