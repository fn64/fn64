#include <assert.h>
#include <stdint.h>
#include <stdio.h>

#include "mupen_devtrace_wire.h"

int main(void) {
    struct fn64_pi_observation rom;
    struct fn64_pi_observation sram;
    struct fn64_pi_observation scratch;
    struct fn64_count_clock clock;
    struct fn64_event_clock event_clock;
    uint64_t master_cycles;
    uint32_t scope;

    assert(fn64_parse_timing_scope("vi,ai,mi", &scope));
    assert(scope == (FN64_SCOPE_VI | FN64_SCOPE_AI | FN64_SCOPE_MI));
    assert(!fn64_parse_timing_scope("", &scope));
    assert(!fn64_parse_timing_scope("vi,vi", &scope));
    assert(!fn64_parse_timing_scope("vi,nope", &scope));

    fn64_count_clock_init(&clock, 0xfffffffeu);
    assert(fn64_count_clock_observe(&clock, 0xfffffffeu, &master_cycles) ==
           FN64_COUNT_CLOCK_OK);
    assert(master_cycles == 0u);
    assert(fn64_count_clock_observe(&clock, 1u, &master_cycles) ==
           FN64_COUNT_CLOCK_OK);
    assert(master_cycles == 6u);
    assert(fn64_count_clock_observe(
               &clock, 1u + FN64_MAX_COUNT_TICKS_PER_DEBUG_STEP + 1u,
               &master_cycles) == FN64_COUNT_CLOCK_DISCONTINUITY);

    assert(fn64_instruction_writes_cp0_count((0x10u << 26) | (0x04u << 21) | (9u << 11)));
    assert(fn64_instruction_writes_cp0_count((0x10u << 26) | (0x05u << 21) | (9u << 11)));
    assert(!fn64_instruction_writes_cp0_count((0x10u << 26) | (0x00u << 21) | (9u << 11)));
    assert(!fn64_instruction_writes_cp0_count((0x10u << 26) | (0x04u << 21) | (11u << 11)));

    fn64_event_clock_init(&event_clock);
    assert(fn64_event_clock_stamp(&event_clock, 100u) == 0u);
    assert(fn64_event_clock_stamp(&event_clock, 112u) == 12u);

    assert(fn64_classify_pi_observation(0x10000010u, 0x00000020u,
                                        FN64_PI_LEN_UNREADABLE, 0x0000003fu,
                                        &rom) == FN64_PI_OBSERVATION_OK);
    assert(rom.direction == FN64_PI_TO_RDRAM);
    assert(rom.device == FN64_PI_ROM);
    assert(rom.device_offset == 0x10u);
    assert(rom.len == 64u);

    assert(fn64_classify_pi_observation(0x08000010u, 0x00000100u,
                                        0x0000001fu, FN64_PI_LEN_UNREADABLE,
                                        &sram) == FN64_PI_OBSERVATION_OK);
    assert(sram.direction == FN64_PI_FROM_RDRAM);
    assert(sram.device == FN64_PI_SRAM);
    assert(sram.device_offset == 0x10u);
    assert(sram.len == 32u);

    assert(fn64_classify_pi_observation(0x10000000u, 0u,
                                        FN64_PI_LEN_UNREADABLE, FN64_PI_LEN_UNREADABLE,
                                        &scratch) == FN64_PI_LENGTH_UNREADABLE);
    assert(fn64_classify_pi_observation(0x10000000u, 0u, 0u, 0u, &scratch) ==
           FN64_PI_LENGTH_AMBIGUOUS);
    assert(fn64_classify_pi_observation(0x1fbffff0u, 0u,
                                        FN64_PI_LEN_UNREADABLE, 0x1fu,
                                        &scratch) == FN64_PI_DEVICE_RANGE_INVALID);
    assert(fn64_classify_pi_observation(0x05000000u, 0u,
                                        FN64_PI_LEN_UNREADABLE, 0u,
                                        &scratch) == FN64_PI_DEVICE_WINDOW_INVALID);
    assert(fn64_pi_observation_error_text(FN64_PI_LENGTH_UNREADABLE)[0] != '\0');
    assert(fn64_pi_completion_is_proven(0u, FN64_MI_INTR_PI));
    assert(!fn64_pi_completion_is_proven(FN64_MI_INTR_PI, FN64_MI_INTR_PI));
    assert(!fn64_pi_completion_is_proven(0u, 0u));
    assert(!fn64_pi_completion_is_proven(0u, 0x00000008u));
    assert(!fn64_pi_completion_is_proven(FN64_MI_INTR_PI, 0u));

    fn64_event_clock_init(&event_clock);
    fn64_emit_timing_header(stdout, "mupen-devtrace v3 source fixture", "source-fixture-1",
                            FN64_SCOPE_PRODUCER_DEFAULT);
    fn64_emit_timing_pi_event(stdout, 1, "dma_start",
                              fn64_event_clock_stamp(&event_clock, 100), &rom);
    fn64_emit_timing_pi_event(stdout, 2, "dma_complete",
                              fn64_event_clock_stamp(&event_clock, 112), &rom);
    fn64_emit_timing_event(stdout, 3, "mi_raise", "mi",
                           fn64_event_clock_stamp(&event_clock, 112), FN64_MI_INTR_PI, 0);
    fn64_emit_timing_pi_event(stdout, 4, "dma_start",
                              fn64_event_clock_stamp(&event_clock, 200), &sram);
    fn64_emit_timing_pi_event(stdout, 5, "dma_complete",
                              fn64_event_clock_stamp(&event_clock, 220), &sram);
    fn64_emit_timing_event(stdout, 6, "dma_start", "si",
                           fn64_event_clock_stamp(&event_clock, 230), 1024, 0);
    fn64_emit_timing_event(stdout, 7, "mi_raise", "mi",
                           fn64_event_clock_stamp(&event_clock, 240), 2, 0);
    fn64_emit_timing_event(stdout, 8, "vi_retrace", "vi",
                           fn64_event_clock_stamp(&event_clock, 250), 0, 0);
    fn64_emit_timing_end(stdout, 9, "completed");
    return 0;
}
