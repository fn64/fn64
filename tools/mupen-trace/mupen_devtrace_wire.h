#ifndef FN64_MUPEN_DEVTRACE_WIRE_H
#define FN64_MUPEN_DEVTRACE_WIRE_H

#include <stdint.h>
#include <stdio.h>

#define FN64_PI_DOM2_A2_START 0x08000000u
#define FN64_PI_DOM2_A2_END   0x10000000u
#define FN64_PI_DOM1_A2_START 0x10000000u
#define FN64_PI_DOM1_A2_END   0x1FC00000u
#define FN64_PI_LEN_UNREADABLE 0x0000007Fu
#define FN64_MI_INTR_PI 0x00000010u

enum fn64_pi_direction {
    FN64_PI_TO_RDRAM,
    FN64_PI_FROM_RDRAM,
};

enum fn64_pi_device {
    FN64_PI_ROM,
    FN64_PI_SRAM,
};

enum fn64_pi_observation_error {
    FN64_PI_OBSERVATION_OK,
    FN64_PI_LENGTH_UNREADABLE,
    FN64_PI_LENGTH_AMBIGUOUS,
    FN64_PI_LENGTH_INVALID,
    FN64_PI_DEVICE_WINDOW_INVALID,
    FN64_PI_DEVICE_RANGE_INVALID,
};

struct fn64_pi_observation {
    enum fn64_pi_direction direction;
    enum fn64_pi_device device;
    uint32_t physical_cart_addr;
    uint32_t dram_addr;
    uint32_t device_offset;
    uint32_t len;
};

static const char *fn64_pi_direction_json(enum fn64_pi_direction direction) {
    return direction == FN64_PI_TO_RDRAM ? "to_rdram" : "from_rdram";
}

static const char *fn64_pi_device_json(enum fn64_pi_device device) {
    return device == FN64_PI_ROM ? "rom" : "sram";
}

static const char *fn64_pi_observation_error_text(enum fn64_pi_observation_error error) {
    switch (error) {
    case FN64_PI_OBSERVATION_OK:
        return "ok";
    case FN64_PI_LENGTH_UNREADABLE:
        return "both PI length registers have the unreadable 0x7f value";
    case FN64_PI_LENGTH_AMBIGUOUS:
        return "both PI length registers claim the same DMA start";
    case FN64_PI_LENGTH_INVALID:
        return "the observed PI length cannot be represented as a nonzero byte count";
    case FN64_PI_DEVICE_WINDOW_INVALID:
        return "PI_CART_ADDR is outside public Domain1/Domain2 Address2 device windows";
    case FN64_PI_DEVICE_RANGE_INVALID:
        return "the complete PI device range crosses its physical Address2 window";
    }
    return "unknown PI observation error";
}

static enum fn64_pi_observation_error fn64_classify_pi_observation(
    uint32_t physical_cart_addr,
    uint32_t dram_addr,
    uint32_t rd_len,
    uint32_t wr_len,
    struct fn64_pi_observation *out) {
    int rd_claims = rd_len != FN64_PI_LEN_UNREADABLE;
    int wr_claims = wr_len != FN64_PI_LEN_UNREADABLE;
    uint32_t encoded_len;
    uint32_t window_start;
    uint32_t window_end;
    uint64_t physical_end;

    if (!rd_claims && !wr_claims)
        return FN64_PI_LENGTH_UNREADABLE;
    if (rd_claims && wr_claims)
        return FN64_PI_LENGTH_AMBIGUOUS;

    encoded_len = wr_claims ? wr_len : rd_len;
    if (encoded_len == UINT32_MAX)
        return FN64_PI_LENGTH_INVALID;

    out->direction = wr_claims ? FN64_PI_TO_RDRAM : FN64_PI_FROM_RDRAM;
    out->physical_cart_addr = physical_cart_addr;
    out->dram_addr = dram_addr;
    out->len = encoded_len + 1u;

    if (physical_cart_addr >= FN64_PI_DOM1_A2_START &&
        physical_cart_addr < FN64_PI_DOM1_A2_END) {
        out->device = FN64_PI_ROM;
        window_start = FN64_PI_DOM1_A2_START;
        window_end = FN64_PI_DOM1_A2_END;
    } else if (physical_cart_addr >= FN64_PI_DOM2_A2_START &&
               physical_cart_addr < FN64_PI_DOM2_A2_END) {
        out->device = FN64_PI_SRAM;
        window_start = FN64_PI_DOM2_A2_START;
        window_end = FN64_PI_DOM2_A2_END;
    } else {
        return FN64_PI_DEVICE_WINDOW_INVALID;
    }

    physical_end = (uint64_t)physical_cart_addr + (uint64_t)out->len;
    if (physical_end > (uint64_t)window_end)
        return FN64_PI_DEVICE_RANGE_INVALID;
    out->device_offset = physical_cart_addr - window_start;
    return FN64_PI_OBSERVATION_OK;
}

/* BUSY can fall because PI_STATUS reset cancelled the transfer. Public
 * debugger polling cannot observe the byte commit itself, so only a newly
 * raised PI interrupt in the same poll proves a normal completion boundary.
 * A previously pending PI interrupt is deliberately insufficient. */
static int fn64_pi_completion_is_proven(uint32_t previous_mi, uint32_t current_mi) {
    return ((current_mi & ~previous_mi) & FN64_MI_INTR_PI) != 0;
}

static void fn64_emit_timing_header(FILE *out, const char *producer, const char *trace_id) {
    fprintf(out,
            "{\"record\":\"header\",\"ordinal\":0,\"schema_version\":2,"
            "\"producer\":\"%s\",\"trace_id\":\"%s\"}\n",
            producer, trace_id);
}

static void fn64_emit_timing_event(FILE *out, uint64_t ordinal, const char *event_kind,
                                   const char *device, uint64_t cycle,
                                   uint32_t addr_or_source, uint32_t value_or_len) {
    fprintf(out,
            "{\"record\":\"device_event\",\"ordinal\":%llu,\"event_kind\":\"%s\","
            "\"device\":\"%s\",\"cycle\":%llu,\"addr_or_source\":%u,"
            "\"value_or_len\":%u,\"dma_direction\":null,\"pi_device\":null,"
            "\"pi_offset\":null}\n",
            (unsigned long long)ordinal, event_kind, device,
            (unsigned long long)cycle, addr_or_source, value_or_len);
}

static void fn64_emit_timing_pi_event(FILE *out, uint64_t ordinal, const char *event_kind,
                                      uint64_t cycle,
                                      const struct fn64_pi_observation *observation) {
    fprintf(out,
            "{\"record\":\"device_event\",\"ordinal\":%llu,\"event_kind\":\"%s\","
            "\"device\":\"pi\",\"cycle\":%llu,\"addr_or_source\":%u,"
            "\"value_or_len\":%u,\"dma_direction\":\"%s\",\"pi_device\":\"%s\","
            "\"pi_offset\":%u}\n",
            (unsigned long long)ordinal, event_kind, (unsigned long long)cycle,
            observation->dram_addr, observation->len,
            fn64_pi_direction_json(observation->direction),
            fn64_pi_device_json(observation->device), observation->device_offset);
}

static void fn64_emit_timing_end(FILE *out, uint64_t ordinal, const char *completion) {
    fprintf(out, "{\"record\":\"end\",\"ordinal\":%llu,\"completion\":\"%s\"}\n",
            (unsigned long long)ordinal, completion);
}

#endif
