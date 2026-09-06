#ifndef FN64_MUPEN_DEVTRACE_WIRE_H
#define FN64_MUPEN_DEVTRACE_WIRE_H

#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define FN64_PI_DOM2_A2_START 0x08000000u
#define FN64_PI_DOM2_A2_END   0x10000000u
#define FN64_PI_DOM1_A2_START 0x10000000u
#define FN64_PI_DOM1_A2_END   0x1FC00000u
#define FN64_PI_LEN_UNREADABLE 0x0000007Fu
#define FN64_MI_INTR_PI 0x00000010u
#define FN64_R4300_MASTER_CLOCK_HZ 93750000ull
#define FN64_MAX_COUNT_TICKS_PER_DEBUG_STEP 1000000u
#define FN64_SCOPE_PI (1u << 0)
#define FN64_SCOPE_AI (1u << 1)
#define FN64_SCOPE_SI (1u << 2)
#define FN64_SCOPE_SP (1u << 3)
#define FN64_SCOPE_DP (1u << 4)
#define FN64_SCOPE_VI (1u << 5)
#define FN64_SCOPE_MI (1u << 6)
#define FN64_SCOPE_PRODUCER_DEFAULT \
    (FN64_SCOPE_PI | FN64_SCOPE_AI | FN64_SCOPE_SI | FN64_SCOPE_VI | FN64_SCOPE_MI)

static int fn64_scope_token(const char *token, size_t len, uint32_t *flag) {
    static const struct {
        const char *name;
        uint32_t flag;
    } tokens[] = {
        {"pi", FN64_SCOPE_PI}, {"ai", FN64_SCOPE_AI}, {"si", FN64_SCOPE_SI},
        {"sp", FN64_SCOPE_SP}, {"dp", FN64_SCOPE_DP}, {"vi", FN64_SCOPE_VI},
        {"mi", FN64_SCOPE_MI},
    };
    size_t index;
    for (index = 0; index < sizeof(tokens) / sizeof(tokens[0]); index++) {
        if (strlen(tokens[index].name) == len &&
            memcmp(tokens[index].name, token, len) == 0) {
            *flag = tokens[index].flag;
            return 1;
        }
    }
    return 0;
}

static int fn64_parse_timing_scope(const char *text, uint32_t *scope) {
    const char *token;
    const char *cursor;
    uint32_t parsed = 0;

    if (text == NULL || *text == '\0')
        return 0;
    token = text;
    cursor = text;
    for (;;) {
        if (*cursor == ',' || *cursor == '\0') {
            uint32_t flag;
            size_t len = (size_t)(cursor - token);
            if (len == 0 || !fn64_scope_token(token, len, &flag) || (parsed & flag) != 0)
                return 0;
            parsed |= flag;
            if (*cursor == '\0')
                break;
            token = cursor + 1;
        }
        cursor++;
    }
    *scope = parsed;
    return 1;
}

enum fn64_count_clock_error {
    FN64_COUNT_CLOCK_OK,
    FN64_COUNT_CLOCK_DISCONTINUITY,
    FN64_COUNT_CLOCK_OVERFLOW,
};

struct fn64_count_clock {
    uint32_t previous_count;
    uint64_t elapsed_count_ticks;
};

struct fn64_event_clock {
    int anchored;
    uint64_t first_master_cycle;
};

static void fn64_event_clock_init(struct fn64_event_clock *clock) {
    clock->anchored = 0;
    clock->first_master_cycle = 0;
}

static uint64_t fn64_event_clock_stamp(
    struct fn64_event_clock *clock,
    uint64_t master_cycle) {
    if (!clock->anchored) {
        clock->anchored = 1;
        clock->first_master_cycle = master_cycle;
    }
    return master_cycle - clock->first_master_cycle;
}

static void fn64_count_clock_init(struct fn64_count_clock *clock, uint32_t count) {
    clock->previous_count = count;
    clock->elapsed_count_ticks = 0;
}

/* CP0 Count is a guest-writable half-rate architectural register, not the
 * monotonic device clock. Within a single-step window where Count writes are
 * rejected, modular deltas unwrap its 32-bit rollover and multiplication by
 * two projects those elapsed ticks into the trace's master-cycle unit. The
 * result has a two-master-cycle quantum because Count does not expose the odd
 * phase. */
static enum fn64_count_clock_error fn64_count_clock_observe(
    struct fn64_count_clock *clock,
    uint32_t count,
    uint64_t *master_cycles) {
    uint32_t delta = count - clock->previous_count;
    uint64_t elapsed;

    if (delta > FN64_MAX_COUNT_TICKS_PER_DEBUG_STEP)
        return FN64_COUNT_CLOCK_DISCONTINUITY;
    if (UINT64_MAX - clock->elapsed_count_ticks < (uint64_t)delta)
        return FN64_COUNT_CLOCK_OVERFLOW;
    elapsed = clock->elapsed_count_ticks + (uint64_t)delta;
    if (elapsed > UINT64_MAX / 2u)
        return FN64_COUNT_CLOCK_OVERFLOW;

    clock->previous_count = count;
    clock->elapsed_count_ticks = elapsed;
    *master_cycles = elapsed * 2u;
    return FN64_COUNT_CLOCK_OK;
}

/* MTC0/DMTC0 rt,Count are the architectural paths that can invalidate the
 * monotonic projection. The public R4300 encoding identifies both without
 * inspecting any reference-runtime implementation. */
static int fn64_instruction_writes_cp0_count(uint32_t instruction) {
    uint32_t opcode = instruction >> 26;
    uint32_t rs = (instruction >> 21) & 0x1fu;
    uint32_t rd = (instruction >> 11) & 0x1fu;
    return opcode == 0x10u && (rs == 0x04u || rs == 0x05u) && rd == 9u;
}

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

static void fn64_emit_timing_header(
    FILE *out,
    const char *producer,
    const char *trace_id,
    uint32_t scope) {
    static const struct {
        const char *name;
        uint32_t flag;
    } devices[] = {
        {"pi", FN64_SCOPE_PI}, {"ai", FN64_SCOPE_AI}, {"si", FN64_SCOPE_SI},
        {"sp", FN64_SCOPE_SP}, {"dp", FN64_SCOPE_DP}, {"vi", FN64_SCOPE_VI},
        {"mi", FN64_SCOPE_MI},
    };
    size_t index;
    int first = 1;
    fprintf(out,
            "{\"record\":\"header\",\"ordinal\":0,\"schema_version\":3,"
            "\"clock\":{\"unit\":\"r4300_master_cycle\",\"hz\":%llu,"
            "\"origin\":\"first_event\",\"quantum\":2},\"observed_devices\":[",
            FN64_R4300_MASTER_CLOCK_HZ);
    for (index = 0; index < sizeof(devices) / sizeof(devices[0]); index++) {
        if ((scope & devices[index].flag) != 0) {
            fprintf(out, "%s\"%s\"", first ? "" : ",", devices[index].name);
            first = 0;
        }
    }
    fprintf(out, "],\"producer\":\"%s\",\"trace_id\":\"%s\"}\n", producer, trace_id);
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
