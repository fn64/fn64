/*
 * mupen_devtrace.c -- fn64 differential-timing-oracle device-event producer
 * (design spec `docs/superpowers/specs/2026-07-23-timing-oracle-design.md`,
 * increment 0a component 1).
 *
 * Drives a DEBUGGER=1 mupen64plus-core in single-step, exactly like
 * `mupen_trace.c` (same dlopen/dlsym public-API pattern, same
 * DebugSetCallbacks/DebugStep/DebugMemRead32 seam -- see that file's header
 * comment for the full API-usage rationale, which is not repeated here).
 * Where `mupen_trace.c` emits `executed_pc` / `watched_table_write` records
 * (fn64-discover's PC trace schema, `crates/fn64-discover/src/trace.rs`),
 * this producer emits `crates/fn64-discover/src/timing_trace.rs`'s
 * cycle-stamped DEVICE-EVENT schema: PI/AI/SI DMA start/complete, MI
 * interrupt raise/ack, VI retrace.
 *
 * ## Wire schema (read from timing_trace.rs -- do not drift from it)
 * JSONL, one record per line, tagged by a `"record"` field
 * (serde `#[serde(tag = "record", rename_all = "snake_case")]`):
 *   - {"record":"header","ordinal":0,"schema_version":3,
 *      "clock":{"unit":"r4300_master_cycle","hz":93750000,
 *      "origin":"first_event","quantum":2},
 *      "observed_devices":["pi","ai","si","vi","mi"],
 *      "producer":"...","trace_id":"..."}
 *   - {"record":"device_event","ordinal":N,"event_kind":"dma_start","device":"pi",
 *      "cycle":123,"addr_or_source":32,"value_or_len":64,
 *      "dma_direction":"to_rdram","pi_device":"rom","pi_offset":16}
 *   - {"record":"end","ordinal":N,"completion":"completed"}
 * `event_kind` in {dma_start, dma_complete, mi_raise, mi_ack, vi_retrace}.
 * `device` in {pi, ai, si, sp, dp, vi, mi}. The header's canonical
 * `observed_devices` list is authoritative. This producer supports
 * pi/ai/si/vi/mi only -- sp/dp timing is out of this increment's scope.
 * Ordinals are dense: header is 0, then one integer per emitted device_event,
 * then end is next. `ingest_jsonl` in timing_trace.rs rejects gaps.
 *
 * ## Register map
 * The KSEG1 (uncached, 0xA4xxxxxx) addresses and status bits below come from
 * the public Nintendo `rcp.h` register definitions and R4300/RCP manuals.
 * Keep them covered by the source-contract test: a reserved-address typo once
 * hid every SI busy edge while still producing plausible MI-only traces.
 * Relevant distinctions include:
 *   - VI_V_INTR_REG is offset 0x0C (index 3), NOT 0x08. VI_WIDTH_REG is 0x08.
 *     (STATUS=0x00, ORIGIN=0x04, WIDTH=0x08, V_INTR=0x0C, CURRENT=0x10.)
 *   - AI_STATUS_BUSY is bit 30 (0x40000000) and AI_STATUS_FULL is bit 31
 *     (0x80000000), i.e. busy=bit30/full=bit31.
 *   - SI_STATUS: DMA_BUSY = bit0 (0x0001), INTERRUPT = bit12 (0x1000)
 *     (`SI_STATUS_REG`).
 *   - MI_INTR source bits: SP=0x01 SI=0x02 AI=0x04 VI=0x08 PI=0x10 DP=0x20
 *     -- identical bit-for-bit to fn64's
 *     `InterruptSource::bit()` (`crates/fn64-runtime/src/device.rs`), so
 *     `addr_or_source` for mi_raise/mi_ack is emitted as this raw mask value,
 *     not a bit index; both producers agree on the encoding by construction.
 *
 * PI_DRAM_ADDR   0xA4600000  PI_CART_ADDR  0xA4600004
 * PI_RD_LEN      0xA4600008  PI_WR_LEN     0xA460000C
 * PI_STATUS      0xA4600010  (bit0 = DMA_BUSY)
 * SI_DRAM_ADDR   0xA4800000
 * SI_STATUS      0xA4800018  (bit0 = DMA_BUSY, bit12 = INTERRUPT)
 * AI_DRAM_ADDR   0xA4500000  AI_LEN        0xA4500004
 * AI_CONTROL     0xA4500008  AI_STATUS     0xA450000C  (bit30=BUSY, bit31=FULL)
 * VI_CURRENT     0xA4400010  VI_V_INTR     0xA440000C  VI_V_SYNC 0xA4400018
 * MI_INTR        0xA4300008  MI_INTR_MASK  0xA430000C
 *
 * ## Guest cycle
 * `DebugGetCPUDataPtr(M64P_CPU_REG_COP0)` returns a pointer to the live
 * `uint32_t cop0[32]` register file; index 9 is CP0 Count
 * (`enum r4300_cp0_registers` in `src/device/r4300/cp0.h`: ..., BADVADDR=8,
 * COUNT=9, ENTRYHI=10, ...). Count increments once every 2 CPU master cycles
 * per the public R4300 manual. Raw Count is therefore NOT directly comparable
 * to fn64's 93.75 MHz `DeviceFabric` stamps. This producer unwraps modular
 * Count deltas and multiplies them by two, then makes the first emitted device
 * event cycle zero so independently started producers share an observable
 * alignment point. Schema v3 records that master-cycle unit and the observer's
 * two-cycle quantum. Any
 * MTC0/DMTC0 write to Count before the first event rebases capture start; a
 * write after any emitted event, an implausible one-step delta, or arithmetic
 * overflow aborts the trace rather than fabricating monotonic time.
 *
 * ## Detection strategy: public debugger callbacks plus per-step polling
 * Like `mupen_trace.c`'s watched-cell poller, this producer re-reads the
 * MMIO registers after every retired instruction (via DebugMemRead32) and
 * emits a record on every observed EDGE:
 *   - PI/SI: DMA_BUSY 0->1 is `dma_start` (addr_or_source = DRAM_ADDR for PI,
 *     DRAM_ADDR for SI since SI has no cart-address register; value_or_len =
 *     WR_LEN+1/RD_LEN+1 for PI, 0 for SI -- SI's PIF window is a fixed 64
 *     bytes and mupen carries no explicit length register for it, mirroring
 *     the fn64-side tap's own `value_or_len: 0` convention for SI in
 *     timing_trace.rs). DMA_BUSY 1->0 is `dma_complete` with the same payload
 *     captured at start. PI additionally carries direction plus an explicit
 *     ROM/SRAM offset. WR_LEN means device-to-RDRAM; RD_LEN means
 *     RDRAM-to-device. The public debugger commonly reads both length
 *     registers as 0x7F. If exactly one direction, a representable nonzero
 *     length, and one complete physical Address2 window cannot be observed,
 *     the producer writes an aborted terminator and exits before emitting a
 *     misleading PI start. Completion reuses the complete typed start
 *     observation rather than re-reading consumed registers. A BUSY falling
 *     edge can also be PI_STATUS reset/cancellation, so completion is emitted
 *     only when the same poll observes a newly raised PI MI interrupt. If PI
 *     was already pending or no new PI edge appears, the producer aborts
 *     rather than relabeling a cancellation as committed bytes.
 *   - AI: AI_STATUS_BUSY edges, same start/complete shape, using AI_DRAM_ADDR
 *     / AI_LEN. NOTE: AI has a 2-deep hardware FIFO (`ai_controller.c`
 *     fifo_push/fifo_pop) -- if a second AI DMA is queued while the first is
 *     still draining, AI_STATUS_BUSY never drops between them and this
 *     poller will not observe an intermediate start/complete pair. This is a
 *     documented limitation of step-granularity busy-bit polling, not a
 *     design defect: it undercounts back-to-back AI DMAs but never
 *     fabricates a boundary that did not occur (`wrong == 0`).
 *   - MI_INTR: bit-for-bit diff against the previous poll. A newly-set bit is
 *     `mi_raise`; a newly-cleared bit is `mi_ack`. `addr_or_source` is the
 *     raw MI_INTR mask bit (see above).
 *   - VI: the public `DebugSetCallbacks` contract invokes its third callback
 *     during every vertical interrupt (Mupen64Plus v2.0 Core Debugger API,
 *     "General Debugger Functions"). The callback increments an atomic
 *     counter; the next debugger pause stamps that interrupt from CP0 Count.
 *     More than one callback between pauses aborts because the public API
 *     cannot assign distinct cycle stamps to those interrupts.
 * Because register state is only visible at pause boundaries, an edge is
 * detected at most one instruction late (i.e. at the cycle count of the
 * FIRST step where the effect is observable), matching the same latency
 * `mupen_trace.c` already accepts for its watched-cell polling.
 *
 * ## Recording window
 * By default, recording starts at the first debugger pause so IPL3/boot device
 * timing remains observable. `FN64_FAST_FORWARD_PC=<aligned-resident-va>`
 * instead discards pauses and callbacks until the pause immediately before
 * that instruction executes, then baselines every observed device and starts
 * the `steps` budget. This is the explicit alignment seam for a recompiled
 * lane that begins at the same resident entry boundary; it does not search or
 * resynchronize event streams after capture.
 *
 * Build (macOS, Homebrew mupen64plus headers):
 *   cc -O2 -Wall -Wextra -o mupen_devtrace mupen_devtrace.c \
 *      -I/opt/homebrew/Cellar/mupen64plus/2.6.0/include -lpthread
 * Run:
 *   ./mupen_devtrace <core.dylib> <rom.z64> <rsp.dylib> <out.jsonl> <steps> <trace_id>
 * Set FN64_DEVICE_TRACE_SCOPE to a unique comma-separated subset of
 * pi,ai,si,vi,mi. Omitted means all five. An excluded device is not polled or
 * claimed; in particular, excluding PI bypasses no PI error -- it makes no PI
 * observation at all.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <dlfcn.h>
#include <pthread.h>
#include <stdatomic.h>
#include <unistd.h>
#include <time.h>

/* Public m64p API headers only -- no core implementation source linked. */
#include <mupen64plus/m64p_types.h>
#include <mupen64plus/m64p_common.h>
#include <mupen64plus/m64p_config.h>
#include <mupen64plus/m64p_frontend.h>
#include <mupen64plus/m64p_debugger.h>

#define RESIDENT_VA_START 0x80000400u
#define RESIDENT_VA_END 0x80800000u
#define MAX_SKIP_STEPS 40000000ull

#include "mupen_devtrace_wire.h"

/* ---- verified RCP register addresses (KSEG1, uncached); see header ---- */
#define PI_DRAM_ADDR 0xA4600000u
#define PI_CART_ADDR 0xA4600004u
#define PI_RD_LEN    0xA4600008u
#define PI_WR_LEN    0xA460000Cu
#define PI_STATUS    0xA4600010u
#define PI_STATUS_DMA_BUSY 0x01u

#define SI_DRAM_ADDR 0xA4800000u
#define SI_STATUS    0xA4800018u
#define SI_STATUS_DMA_BUSY 0x0001u

#define AI_DRAM_ADDR 0xA4500000u
#define AI_LEN       0xA4500004u
#define AI_STATUS    0xA450000Cu
#define AI_STATUS_BUSY 0x40000000u

#define MI_INTR      0xA4300008u

/* Give up if the process wedges before a single pause report arrives. */
#define MAX_INIT_WAIT_MS 20000

static ptr_DebugMemRead32 DebugMemRead32;
static ptr_DebugSetRunState DebugSetRunState;
static ptr_DebugStep DebugStep;
static ptr_DebugGetCPUDataPtr DebugGetCPUDataPtr;
static ptr_CoreDoCommand g_do_command;

/* ---- update-callback -> main-thread handshake (identical pattern to
 * mupen_trace.c: the callback runs on the emulation thread, hands off the
 * paused PC, and blocks until the main thread has consumed it so no pause
 * report is ever silently dropped). ---- */
static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t g_cond = PTHREAD_COND_INITIALIZER;
static pthread_cond_t g_consumed = PTHREAD_COND_INITIALIZER;
static uint32_t g_pc;
static int g_have_pc;
static volatile int g_done;
static volatile int g_dbg_ready;
static _Atomic uint64_t g_vi_callbacks;

static void dbg_init(void) { g_dbg_ready = 1; }
static void dbg_vi(void) {
    /* Closes the emulation-thread VI callback / main-thread pause-sample
     * interleaving: the callback is sequenced before dbg_update's mutex
     * handoff, while this atomic prevents a C data race on the count itself.
     * The main thread still assigns the cycle only after consuming that exact
     * pause; it never guesses a timestamp inside this callback. */
    atomic_fetch_add_explicit(&g_vi_callbacks, UINT64_C(1), memory_order_relaxed);
}

static void dbg_update(unsigned int pc) {
    if (g_done)
        return;
    pthread_mutex_lock(&g_lock);
    while (g_have_pc && !g_done)
        pthread_cond_wait(&g_consumed, &g_lock);
    if (!g_done) {
        g_pc = pc;
        g_have_pc = 1;
        pthread_cond_signal(&g_cond);
    }
    pthread_mutex_unlock(&g_lock);
}

static void debug_cb(void *ctx, int level, const char *msg) {
    (void)ctx;
    fprintf(stderr, "[core lvl=%d] %s\n", level, msg);
}

static void state_cb(void *ctx, m64p_core_param param, int value) {
    (void)ctx;
    (void)param;
    (void)value;
}

static void *exec_trampoline(void *a) {
    (void)a;
    g_do_command(M64CMD_EXECUTE, 0, NULL);
    return NULL;
}

static uint64_t g_kicks;

/* Bounded timed wait re-kicking DebugStep(); see mupen_trace.c's
 * wait_for_pause for the same rationale (undocumented pre/post-report
 * ordering in the public API). */
static void wait_for_pause(uint32_t *pc_out) {
    pthread_mutex_lock(&g_lock);
    while (!g_have_pc) {
        struct timespec deadline;
        clock_gettime(CLOCK_REALTIME, &deadline);
        deadline.tv_nsec += 200 * 1000 * 1000;
        if (deadline.tv_nsec >= 1000000000) {
            deadline.tv_sec += 1;
            deadline.tv_nsec -= 1000000000;
        }
        if (pthread_cond_timedwait(&g_cond, &g_lock, &deadline) != 0 && !g_have_pc) {
            pthread_mutex_unlock(&g_lock);
            g_kicks++;
            DebugStep();
            pthread_mutex_lock(&g_lock);
        }
    }
    *pc_out = g_pc;
    g_have_pc = 0;
    pthread_cond_signal(&g_consumed);
    pthread_mutex_unlock(&g_lock);
}

/* ---- Headless-bridge emission, matching headless.rs's
 * HeadlessObservationRecord exactly (serde tag = "event", rename_all =
 * "snake_case", deny_unknown_fields -- so an extra field is a hard reject,
 * not a warning). Selected by passing a run-bundle SHA-256 as an 8th
 * argument; without it this producer's timing output is byte-identical to
 * what it always emitted.
 *
 * Unlike the timing schema (two payload fields, sized for cycle stamps) this
 * carries the full DMA triple, which is what
 * trace::fold_pi_dmas_into_fact_db needs to conclude a load-image mapping. ---- */

static void emit_hl_header(FILE *out, const char *trace_id, const char *bundle_sha256) {
    fprintf(out,
            "{\"event\":\"header\",\"sequence\":0,\"schema_version\":1,"
            "\"trace_id\":\"%s\",\"run_bundle_sha256\":\"%s\"}\n",
            trace_id, bundle_sha256);
}

static void emit_hl_pi_dma(FILE *out, uint64_t sequence, const char *probe_id,
                            uint32_t cart, uint32_t dram, uint32_t len) {
    /* active_bank is Unknown: this producer observes the device, not the
     * CPU's current bank identity, and headless.rs preserves unknown-bank
     * identity rather than letting a producer guess one. */
    fprintf(out,
            "{\"event\":\"pi_dma_completed\",\"sequence\":%llu,\"probe_id\":\"%s\","
            "\"direction\":\"cart_to_rdram\",\"cart_address\":%u,\"dram_address\":%u,"
            "\"byte_len\":%u,\"active_bank\":{\"status\":\"unknown\"}}\n",
            (unsigned long long)sequence, probe_id, cart, dram, len);
}

static void emit_hl_end(FILE *out, uint64_t sequence, const char *reason_json,
                         uint64_t instructions, uint64_t time_ns) {
    fprintf(out,
            "{\"event\":\"end\",\"sequence\":%llu,\"stop_reason\":%s,"
            "\"instructions_executed\":%llu,\"emulated_time_ns\":%llu}\n",
            (unsigned long long)sequence, reason_json,
            (unsigned long long)instructions, (unsigned long long)time_ns);
}

/* The core-side PI DMA emitter (FN64_PI_DMA_TRACE, a patch on the pinned
 * mupen fork) flushes every record, but this process exits via _exit(), which
 * runs no atexit handler -- so the core never writes the stream's terminating
 * `end` record and normalize rejects the file as truncated. The launcher knows
 * exactly when the run stopped, so it writes the terminator.
 *
 * The next sequence number is the record count already in the file: the header
 * is sequence 0 and every record increments by one, so a line count is the
 * next value by construction. */
static void fn64_append_end_record(void) {
    const char *path = getenv("FN64_PI_DMA_TRACE");
    if (path == NULL || path[0] == '\0')
        return;
    FILE *count = fopen(path, "r");
    if (count == NULL)
        return;
    /* Idempotent: the core registers an atexit handler that writes this same
     * record, and whether it runs depends on dlopen/dlclose teardown order,
     * which is not something to build on. Writing unconditionally produced two
     * terminators ("record appears after end"); trusting atexit produced none
     * ("missing end record"). So: look at what is actually in the file.
     *
     * The last line is scanned for the end event rather than parsed -- the
     * consumer validates the schema, this only needs to know whether a
     * terminator is already present. */
    unsigned long long lines = 0;
    int c;
    char tail[256];
    size_t tail_len = 0;
    while ((c = fgetc(count)) != EOF) {
        if (c == '\n') {
            lines++;
            tail_len = 0;
        } else if (tail_len + 1 < sizeof(tail)) {
            tail[tail_len++] = (char)c;
        }
    }
    /* Re-read the final complete line. */
    fseek(count, 0, SEEK_SET);
    char line[4096];
    int already_terminated = 0;
    while (fgets(line, sizeof(line), count) != NULL)
        already_terminated = (strstr(line, "\"event\":\"end\"") != NULL);
    fclose(count);
    if (already_terminated) {
        fprintf(stderr, "fn64 PI DMA trace: %llu records, already terminated\n", lines - 1);
        return;
    }
    count = NULL;
    FILE *append = fopen(path, "a");
    if (append == NULL) {
        fprintf(stderr, "warning: cannot append end record to %s\n", path);
        return;
    }
    fprintf(append,
            "{\"event\":\"end\",\"sequence\":%llu,\"stop_reason\":{\"reason\":"
            "\"producer_abort\",\"detail\":\"producer step limit reached\"},"
            "\"instructions_executed\":0,\"emulated_time_ns\":0}\n",
            lines);
    fclose(append);
    fprintf(stderr, "fn64 PI DMA trace: %llu records, end record appended\n", lines - 1);
}

/* ---- per-device previous-poll state, for edge detection ---- */
struct pi_state {
    int busy;
    /* Completion must retain the exact direction/device/offset tuple captured
     * at the rising edge; consumed registers are not evidence for it. */
    struct fn64_pi_observation observation;
};
struct si_state {
    int busy;
    uint32_t dram_addr;
};
struct ai_state {
    int busy;
    uint32_t dram_addr;
    uint32_t len;
};

int main(int argc, char **argv) {
    if (argc != 7 && argc != 8) {
        fprintf(stderr,
                "usage: %s <core.dylib> <rom.z64> <rsp.dylib> <out.jsonl> <steps> <trace_id> "
                "[run_bundle_sha256]\n"
                "  with run_bundle_sha256: emit headless-bridge observations "
                "(feed to `headless-bridge normalize`)\n"
                "  without it:            emit the timing device-event schema, unchanged\n",
                argv[0]);
        return 2;
    }
    /* NULL selects the timing schema, so existing invocations are untouched. */
    const char *bundle_sha256 = (argc == 8) ? argv[7] : NULL;
    const char *core_path = argv[1];
    const char *rom_path = argv[2];
    const char *rsp_path = argv[3];
    const char *out_path = argv[4];
    unsigned long long steps = strtoull(argv[5], NULL, 10);
    const char *trace_id = argv[6];
    const char *fast_forward_pc_env = getenv("FN64_FAST_FORWARD_PC");
    uint32_t fast_forward_pc = 0;
    int fast_forward = 0;
    uint32_t timing_scope = FN64_SCOPE_PRODUCER_DEFAULT;
    const char *timing_scope_env = getenv("FN64_DEVICE_TRACE_SCOPE");
    if (steps == 0) {
        fprintf(stderr, "steps must be > 0\n");
        return 2;
    }
    if (fast_forward_pc_env != NULL && fast_forward_pc_env[0] != '\0') {
        char *end = NULL;
        unsigned long parsed = strtoul(fast_forward_pc_env, &end, 0);
        if (end == fast_forward_pc_env || *end != '\0' || parsed > UINT32_MAX
            || (parsed & 3U) != 0 || parsed < RESIDENT_VA_START
            || parsed >= RESIDENT_VA_END) {
            fprintf(stderr, "FN64_FAST_FORWARD_PC must be an aligned resident VA\n");
            return 2;
        }
        fast_forward_pc = (uint32_t)parsed;
        fast_forward = 1;
    }
    if (timing_scope_env != NULL) {
        if (bundle_sha256 != NULL) {
            fprintf(stderr, "FN64_DEVICE_TRACE_SCOPE applies only to timing-schema output\n");
            return 2;
        }
        if (!fn64_parse_timing_scope(timing_scope_env, &timing_scope)
            || (timing_scope & (FN64_SCOPE_SP | FN64_SCOPE_DP)) != 0) {
            fprintf(stderr,
                    "FN64_DEVICE_TRACE_SCOPE must be a unique comma-separated subset of "
                    "pi,ai,si,vi,mi\n");
            return 2;
        }
    }

    /* ---- ROM bytes (native big-endian .z64 only; no byteswap guess) ---- */
    FILE *f = fopen(rom_path, "rb");
    if (!f) {
        fprintf(stderr, "cannot open rom %s\n", rom_path);
        return 1;
    }
    fseek(f, 0, SEEK_END);
    long romlen = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (romlen < 0x40 || (romlen & 3) != 0) {
        fprintf(stderr, "rom length %ld is not a normalizable N64 image\n", romlen);
        return 1;
    }
    unsigned char *rombuf = malloc((size_t)romlen);
    if (!rombuf || fread(rombuf, 1, (size_t)romlen, f) != (size_t)romlen) {
        fprintf(stderr, "short rom read\n");
        return 1;
    }
    fclose(f);
    if (!(rombuf[0] == 0x80 && rombuf[1] == 0x37 && rombuf[2] == 0x12 && rombuf[3] == 0x40)) {
        fprintf(stderr,
                "rom is not native big-endian .z64 (magic %02x %02x %02x %02x); "
                "refusing to guess a byte order\n",
                rombuf[0], rombuf[1], rombuf[2], rombuf[3]);
        return 1;
    }

    /* ---- core + debug API via dlopen/dlsym ---- */
    void *core = dlopen(core_path, RTLD_NOW | RTLD_LOCAL);
    if (!core) {
        fprintf(stderr, "dlopen core failed: %s\n", dlerror());
        return 1;
    }
    ptr_CoreStartup CoreStartup = (ptr_CoreStartup)dlsym(core, "CoreStartup");
    ptr_CoreAttachPlugin CoreAttachPlugin = (ptr_CoreAttachPlugin)dlsym(core, "CoreAttachPlugin");
    ptr_CoreDoCommand CoreDoCommand = (ptr_CoreDoCommand)dlsym(core, "CoreDoCommand");
    ptr_CoreErrorMessage CoreErrorMessage = (ptr_CoreErrorMessage)dlsym(core, "CoreErrorMessage");
    ptr_DebugSetCallbacks DebugSetCallbacks =
        (ptr_DebugSetCallbacks)dlsym(core, "DebugSetCallbacks");
    DebugSetRunState = (ptr_DebugSetRunState)dlsym(core, "DebugSetRunState");
    DebugStep = (ptr_DebugStep)dlsym(core, "DebugStep");
    DebugMemRead32 = (ptr_DebugMemRead32)dlsym(core, "DebugMemRead32");
    DebugGetCPUDataPtr = (ptr_DebugGetCPUDataPtr)dlsym(core, "DebugGetCPUDataPtr");
    if (!CoreStartup || !CoreAttachPlugin || !CoreDoCommand || !DebugSetCallbacks ||
        !DebugSetRunState || !DebugStep || !DebugMemRead32 || !DebugGetCPUDataPtr) {
        fprintf(stderr, "missing required symbol(s) -- was this core built with DEBUGGER=1?\n");
        return 1;
    }

    m64p_error rc = CoreStartup(0x020001, NULL, NULL, NULL, debug_cb, NULL, state_cb);
    if (rc != M64ERR_SUCCESS) {
        fprintf(stderr, "CoreStartup -> %d (%s)\n", rc,
                CoreErrorMessage ? CoreErrorMessage(rc) : "?");
        return 1;
    }
    ptr_ConfigOpenSection ConfigOpenSection =
        (ptr_ConfigOpenSection)dlsym(core, "ConfigOpenSection");
    ptr_ConfigSetParameter ConfigSetParameter =
        (ptr_ConfigSetParameter)dlsym(core, "ConfigSetParameter");
    if (!ConfigOpenSection || !ConfigSetParameter) {
        fprintf(stderr, "missing config API symbols\n");
        return 1;
    }
    /* Full-speed capture. The core-side PI DMA emitter observes transfers from
     * inside dma_pi_write(), so nothing here needs the debugger -- and the
     * debugger is what forced the pure interpreter (EnableDebugger requires
     * R4300Emulator=0). Dropping it moves a capture from ~190k single-steps/sec
     * to dynarec speed, which is the difference between seconds of emulated
     * time and minutes of it. Selected by FN64_CAPTURE_SECONDS, a wall-clock
     * budget, because without single-stepping there is no step counter to
     * bound. */
    const char *seconds_env = getenv("FN64_CAPTURE_SECONDS");
    long capture_seconds = (seconds_env && seconds_env[0]) ? strtol(seconds_env, NULL, 10) : 0;
    int full_speed = capture_seconds > 0;
    if (full_speed && fast_forward) {
        fprintf(stderr,
                "FN64_FAST_FORWARD_PC requires the single-step public-debugger mode\n");
        return 2;
    }
    if (full_speed && getenv("FN64_PI_DMA_TRACE") == NULL) {
        fprintf(stderr,
                "FN64_CAPTURE_SECONDS is set but FN64_PI_DMA_TRACE is not: full-speed mode "
                "observes nothing without the core-side emitter, and this producer's own "
                "polling needs the debugger it disables\n");
        return 2;
    }

    m64p_handle core_section = NULL;
    int one = 1;
    int zero = 0;
    int interpreter = 0; /* pure interpreter: deterministic, debuggable */
    int dynarec = 2;
    if (ConfigOpenSection("Core", &core_section) != M64ERR_SUCCESS ||
        ConfigSetParameter(core_section, "EnableDebugger", M64TYPE_BOOL,
                           full_speed ? &zero : &one) != M64ERR_SUCCESS ||
        ConfigSetParameter(core_section, "R4300Emulator", M64TYPE_INT,
                           full_speed ? &dynarec : &interpreter) != M64ERR_SUCCESS) {
        fprintf(stderr, "failed to set [Core] EnableDebugger/R4300Emulator\n");
        return 1;
    }

    /* Arm the debugger BEFORE ROM open (documented ordering requirement). */
    if (!full_speed)
        DebugSetCallbacks(dbg_init, dbg_update, dbg_vi);

    rc = CoreDoCommand(M64CMD_ROM_OPEN, (int)romlen, rombuf);
    if (rc != M64ERR_SUCCESS) {
        fprintf(stderr, "ROM_OPEN -> %d\n", rc);
        return 1;
    }
    void *rsp_h = dlopen(rsp_path, RTLD_NOW | RTLD_LOCAL);
    if (!rsp_h) {
        fprintf(stderr, "dlopen rsp failed: %s\n", dlerror());
        return 1;
    }
    ptr_PluginStartup PluginStartup = (ptr_PluginStartup)dlsym(rsp_h, "PluginStartup");
    if (!PluginStartup || PluginStartup(core, NULL, debug_cb) != M64ERR_SUCCESS ||
        CoreAttachPlugin(M64PLUGIN_RSP, rsp_h) != M64ERR_SUCCESS) {
        fprintf(stderr, "rsp plugin attach failed\n");
        return 1;
    }
    if (CoreAttachPlugin(M64PLUGIN_GFX, NULL) != M64ERR_SUCCESS ||
        CoreAttachPlugin(M64PLUGIN_AUDIO, NULL) != M64ERR_SUCCESS ||
        CoreAttachPlugin(M64PLUGIN_INPUT, NULL) != M64ERR_SUCCESS) {
        fprintf(stderr, "dummy plugin attach failed\n");
        return 1;
    }

    FILE *out = fopen(out_path, "wb");
    if (!out) {
        fprintf(stderr, "cannot open output %s\n", out_path);
        return 1;
    }

    g_do_command = CoreDoCommand;
    pthread_t exec_thread;
    pthread_create(&exec_thread, NULL, exec_trampoline, NULL);

    if (full_speed) {
        /* Nothing to poll: the core writes the observation stream itself. Let
         * the dynarec run for the wall-clock budget, stop, and terminate the
         * stream. */
        fprintf(stderr,
                "full-speed capture: running %ld s with the dynarec, no debugger\n"
                "  WARNING: this mode is NOT deterministic for every ROM. The budget is\n"
                "  wall-clock, not a step count, so how far a title gets depends on host\n"
                "  scheduling. Measured: Super Mario 64 reproduced byte-identically over\n"
                "  three runs and GoldenEye over two, but Perfect Dark produced 2 and 240\n"
                "  transfers on two runs of the same budget. Verify reproducibility per ROM\n"
                "  before treating this output as evidence; the single-step mode is bounded\n"
                "  by a step count and is deterministic by construction.\n",
                capture_seconds);
        struct timespec budget;
        budget.tv_sec = capture_seconds;
        budget.tv_nsec = 0;
        nanosleep(&budget, NULL);
        CoreDoCommand(M64CMD_STOP, 0, NULL);
        pthread_join(exec_thread, NULL);
        fclose(out);
        fn64_append_end_record();
        return 0;
    }

    for (int waited_ms = 0; !g_dbg_ready; waited_ms += 10) {
        if (waited_ms > MAX_INIT_WAIT_MS) {
            fprintf(stderr, "debugger init callback never fired; aborting\n");
            _exit(1);
        }
        usleep(10 * 1000);
    }
    rc = DebugSetRunState(M64P_DBG_RUNSTATE_STEPPING);
    if (rc != M64ERR_SUCCESS) {
        fprintf(stderr, "DebugSetRunState(STEPPING) -> %d; aborting\n", rc);
        _exit(1);
    }
    DebugStep();

    char producer[192];
    snprintf(producer, sizeof(producer),
             "mupen-devtrace v3 (mupen64plus-core DEBUGGER=1 pure-interpreter + rsp plugin, "
             "single-step register polling + vertical-interrupt callback via public "
             "m64p_debugger API)");
    if (bundle_sha256)
        emit_hl_header(out, trace_id, bundle_sha256);
    else
        fn64_emit_timing_header(out, producer, trace_id, timing_scope);
    uint64_t ordinal = 1;

    /* First pause establishes the capture boundary. An explicit start PC is
     * matched only at a debugger pause, which is the public API's state before
     * that instruction executes. No device state is sampled before the match,
     * so the later baseline cannot manufacture a pre-window edge. */
    uint32_t pc;
    wait_for_pause(&pc);
    unsigned long long skipped = 0;
    while (fast_forward && pc != fast_forward_pc) {
        if (++skipped > MAX_SKIP_STEPS) {
            fprintf(stderr,
                    "FN64_FAST_FORWARD_PC 0x%08x was not reached within %llu steps\n",
                    fast_forward_pc, (unsigned long long)MAX_SKIP_STEPS);
            if (bundle_sha256)
                emit_hl_end(out, ordinal,
                            "{\"reason\":\"producer_abort\",\"detail\":"
                            "\"fast-forward pc not reached\"}",
                            0, 0);
            else
                fn64_emit_timing_end(out, ordinal, "aborted");
            fclose(out);
            _exit(3);
        }
        DebugStep();
        wait_for_pause(&pc);
    }
    if (fast_forward) {
        fprintf(stderr,
                "device timing capture starts before 0x%08x after %llu pre-window steps\n",
                fast_forward_pc, skipped);
    }

    uint32_t *cop0 = (uint32_t *)DebugGetCPUDataPtr(M64P_CPU_REG_COP0);
    if (!cop0) {
        fprintf(stderr, "DebugGetCPUDataPtr(COP0) returned NULL\n");
        _exit(1);
    }
    struct fn64_count_clock count_clock;
    struct fn64_event_clock event_clock;
    fn64_count_clock_init(&count_clock, cop0[9]);
    fn64_event_clock_init(&event_clock);
    int rebase_count_clock = 0;

    struct pi_state pi_prev;
    struct si_state si_prev;
    struct ai_state ai_prev;
    memset(&pi_prev.observation, 0, sizeof(pi_prev.observation));
    pi_prev.busy = (timing_scope & FN64_SCOPE_PI) != 0
        ? DebugMemRead32(PI_STATUS) & PI_STATUS_DMA_BUSY
        : 0;
    si_prev.busy = (timing_scope & FN64_SCOPE_SI) != 0
        ? DebugMemRead32(SI_STATUS) & SI_STATUS_DMA_BUSY
        : 0;
    si_prev.dram_addr = (timing_scope & FN64_SCOPE_SI) != 0
        ? DebugMemRead32(SI_DRAM_ADDR)
        : 0;
    ai_prev.busy = (timing_scope & FN64_SCOPE_AI) != 0
        ? (DebugMemRead32(AI_STATUS) & AI_STATUS_BUSY) != 0
        : 0;
    ai_prev.dram_addr = (timing_scope & FN64_SCOPE_AI) != 0
        ? DebugMemRead32(AI_DRAM_ADDR)
        : 0;
    ai_prev.len = (timing_scope & FN64_SCOPE_AI) != 0 ? DebugMemRead32(AI_LEN) : 0;
    uint32_t mi_prev = DebugMemRead32(MI_INTR);
    /* Capture begins at the first debugger pause, like every polled device.
     * A VI callback before that boundary belongs to the unobserved prelude. */
    uint64_t vi_callbacks_prev =
        atomic_load_explicit(&g_vi_callbacks, memory_order_relaxed);

    if ((timing_scope & FN64_SCOPE_PI) != 0 && pi_prev.busy) {
        fprintf(stderr,
                "FATAL: PI DMA was already busy at the first debugger pause; its start "
                "boundary and typed identity were not observed. Refusing to emit a "
                "completion without its start.\n");
        if (bundle_sha256)
            emit_hl_end(out, ordinal,
                        "{\"reason\":\"producer_abort\",\"detail\":"
                        "\"pi dma start preceded debugger baseline\"}",
                        0, 0);
        else
            fn64_emit_timing_end(out, ordinal, "aborted");
        fclose(out);
        _exit(3);
    }

    unsigned long long recorded = 0;
    for (;;) {
        uint32_t count_now = cop0[9];
        uint64_t cycle;
        enum fn64_count_clock_error clock_error;
        if (rebase_count_clock) {
            fn64_count_clock_init(&count_clock, count_now);
            rebase_count_clock = 0;
        }
        clock_error = fn64_count_clock_observe(&count_clock, count_now, &cycle);
        if (clock_error != FN64_COUNT_CLOCK_OK && ordinal == 1) {
            fn64_count_clock_init(&count_clock, count_now);
            cycle = 0;
            clock_error = FN64_COUNT_CLOCK_OK;
        }
        if (clock_error != FN64_COUNT_CLOCK_OK) {
            fprintf(stderr,
                    "FATAL: CP0 Count cannot be projected into monotonic master cycles "
                    "at step %llu (error=%d previous=0x%08x current=0x%08x). "
                    "Refusing to emit a fabricated timestamp.\n",
                    recorded, (int)clock_error, count_clock.previous_count, count_now);
            if (bundle_sha256)
                emit_hl_end(out, ordinal,
                            "{\"reason\":\"producer_abort\",\"detail\":"
                            "\"cp0 count clock discontinuity\"}",
                            recorded, 0);
            else
                fn64_emit_timing_end(out, ordinal, "aborted");
            fclose(out);
            _exit(3);
        }
        /* Sample MI before classifying PI's BUSY edge. A newly raised PI bit
         * in this exact poll is the only public-debugger evidence that a
         * falling BUSY edge was completion rather than PI_STATUS reset. The
         * MI emission loop below consumes the same sample after PI, retaining
         * the canonical PI-complete-then-MI-raise record order. */
        uint32_t mi_now = DebugMemRead32(MI_INTR);

        /* ---- PI DMA edges ---- */
        if ((timing_scope & FN64_SCOPE_PI) != 0) {
            uint32_t pi_status = DebugMemRead32(PI_STATUS);
            int pi_busy = (pi_status & PI_STATUS_DMA_BUSY) != 0;
            if (pi_busy && !pi_prev.busy) {
                /* Rising edge: sample every public register once at the first
                 * pause where BUSY is visible. The classifier accepts only one
                 * direction claim and one complete physical Address2 range. The
                 * common 0x7F/0x7F readback therefore aborts before emission; it
                 * is not silently promoted into invented transfer geometry. */
                uint32_t rd_len = DebugMemRead32(PI_RD_LEN);
                uint32_t wr_len = DebugMemRead32(PI_WR_LEN);
                uint32_t cart_addr = DebugMemRead32(PI_CART_ADDR);
                uint32_t dram_addr = DebugMemRead32(PI_DRAM_ADDR);
                enum fn64_pi_observation_error observation_error =
                    fn64_classify_pi_observation(cart_addr, dram_addr, rd_len, wr_len,
                                                 &pi_prev.observation);
                if (observation_error != FN64_PI_OBSERVATION_OK) {
                    fprintf(stderr,
                            "FATAL: cannot classify PI DMA start at cart 0x%08x / dram 0x%08x "
                            "from public debugger values RD_LEN=0x%08x WR_LEN=0x%08x: %s. "
                            "Refusing to emit a misleading PI start.\n",
                            cart_addr, dram_addr, rd_len, wr_len,
                            fn64_pi_observation_error_text(observation_error));
                    if (bundle_sha256)
                        emit_hl_end(out, ordinal,
                                    "{\"reason\":\"producer_abort\",\"detail\":"
                                    "\"pi dma identity unreadable via debugger path\"}",
                                    recorded, 0);
                    else
                        fn64_emit_timing_end(out, ordinal, "aborted");
                    fclose(out);
                    _exit(3);
                }
                if (!bundle_sha256)
                    fn64_emit_timing_pi_event(out, ordinal++, "dma_start",
                                              fn64_event_clock_stamp(&event_clock, cycle),
                                              &pi_prev.observation);
            } else if (!pi_busy && pi_prev.busy) {
                if (!fn64_pi_completion_is_proven(mi_prev, mi_now)) {
                    fprintf(stderr,
                            "FATAL: PI BUSY cleared without a newly raised PI MI interrupt in "
                            "the same debugger poll. The transfer may have been reset/cancelled, "
                            "or PI was already pending; refusing to emit a fabricated completion.\n");
                    if (bundle_sha256)
                        emit_hl_end(out, ordinal,
                                    "{\"reason\":\"producer_abort\",\"detail\":"
                                    "\"pi completion not proven by a new interrupt edge\"}",
                                    recorded, 0);
                    else
                        fn64_emit_timing_end(out, ordinal, "aborted");
                    fclose(out);
                    _exit(3);
                }
                if (bundle_sha256) {
                    /* Falling edge only. headless.rs names this record
                     * PiDmaCompleted deliberately: a register write or a
                     * DMA-start notification is explicitly NOT sufficient
                     * evidence, because a started transfer need not complete
                     * with the geometry it started with. */
                    if (pi_prev.observation.direction == FN64_PI_TO_RDRAM)
                        emit_hl_pi_dma(out, ordinal++, "pi_dma_loads",
                                       pi_prev.observation.physical_cart_addr,
                                       pi_prev.observation.dram_addr,
                                       pi_prev.observation.len);
                } else {
                    fn64_emit_timing_pi_event(out, ordinal++, "dma_complete",
                                              fn64_event_clock_stamp(&event_clock, cycle),
                                              &pi_prev.observation);
                }
            }
            pi_prev.busy = pi_busy;
        }

        /* ---- SI DMA edges (fixed 64-byte PIF window; no length register,
         * mirroring the fn64-side tap's value_or_len=0 convention for SI) ---- */
        if ((timing_scope & FN64_SCOPE_SI) != 0) {
            uint32_t si_status = DebugMemRead32(SI_STATUS);
            int si_busy = (si_status & SI_STATUS_DMA_BUSY) != 0;
            if (si_busy && !si_prev.busy) {
                si_prev.dram_addr = DebugMemRead32(SI_DRAM_ADDR);
                if (!bundle_sha256)
                    fn64_emit_timing_event(out, ordinal++, "dma_start", "si",
                                           fn64_event_clock_stamp(&event_clock, cycle),
                                           si_prev.dram_addr, 0);
            } else if (!si_busy && si_prev.busy) {
                if (!bundle_sha256)
                    fn64_emit_timing_event(out, ordinal++, "dma_complete", "si",
                                           fn64_event_clock_stamp(&event_clock, cycle),
                                           si_prev.dram_addr, 0);
            }
            si_prev.busy = si_busy;
        }

        /* ---- AI DMA edges (2-deep FIFO caveat: see header comment) ---- */
        if ((timing_scope & FN64_SCOPE_AI) != 0) {
            uint32_t ai_status = DebugMemRead32(AI_STATUS);
            int ai_busy = (ai_status & AI_STATUS_BUSY) != 0;
            if (ai_busy && !ai_prev.busy) {
                ai_prev.dram_addr = DebugMemRead32(AI_DRAM_ADDR);
                ai_prev.len = DebugMemRead32(AI_LEN);
                if (!bundle_sha256)
                    fn64_emit_timing_event(out, ordinal++, "dma_start", "ai",
                                           fn64_event_clock_stamp(&event_clock, cycle),
                                           ai_prev.dram_addr, ai_prev.len);
            } else if (!ai_busy && ai_prev.busy) {
                if (!bundle_sha256)
                    fn64_emit_timing_event(out, ordinal++, "dma_complete", "ai",
                                           fn64_event_clock_stamp(&event_clock, cycle),
                                           ai_prev.dram_addr, ai_prev.len);
            }
            ai_prev.busy = ai_busy;
        }

        /* ---- VI retrace: the documented public callback fires during each
         * vertical interrupt. A pause can timestamp one such callback, but
         * cannot recover separate timestamps if multiple interrupts elapsed. ---- */
        if (bundle_sha256 == NULL && (timing_scope & FN64_SCOPE_VI) != 0) {
            uint64_t vi_callbacks_now =
                atomic_load_explicit(&g_vi_callbacks, memory_order_relaxed);
            if (vi_callbacks_now < vi_callbacks_prev
                || vi_callbacks_now - vi_callbacks_prev > UINT64_C(1)) {
                uint64_t callback_delta = vi_callbacks_now >= vi_callbacks_prev
                    ? vi_callbacks_now - vi_callbacks_prev
                    : UINT64_MAX;
                fprintf(stderr,
                        "FATAL: %llu vertical-interrupt callbacks occurred between "
                        "debugger pauses at step %llu. The public API cannot assign "
                        "distinct cycle stamps; refusing to fabricate VI timing.\n",
                        (unsigned long long)callback_delta,
                        recorded);
                fn64_emit_timing_end(out, ordinal, "aborted");
                fclose(out);
                _exit(3);
            }
            if (vi_callbacks_now != vi_callbacks_prev)
                fn64_emit_timing_event(out, ordinal++, "vi_retrace", "vi",
                                       fn64_event_clock_stamp(&event_clock, cycle), 0, 0);
            vi_callbacks_prev = vi_callbacks_now;
        }

        /* ---- MI interrupt raise/ack: bit-for-bit diff. VI is emitted first
         * when its callback and MI bit become visible at the same pause. ---- */
        uint32_t raised = mi_now & ~mi_prev;
        uint32_t acked = mi_prev & ~mi_now;
        for (uint32_t bit = 0x01; bit <= 0x20; bit <<= 1) {
            if ((timing_scope & FN64_SCOPE_MI) != 0 && (raised & bit) != 0)
                if (!bundle_sha256)
                    fn64_emit_timing_event(out, ordinal++, "mi_raise", "mi",
                                           fn64_event_clock_stamp(&event_clock, cycle), bit, 0);
            if ((timing_scope & FN64_SCOPE_MI) != 0 && (acked & bit) != 0)
                if (!bundle_sha256)
                    fn64_emit_timing_event(out, ordinal++, "mi_ack", "mi",
                                           fn64_event_clock_stamp(&event_clock, cycle), bit, 0);
        }
        mi_prev = mi_now;

        recorded++;
        if (recorded >= steps) {
            if (bundle_sha256)
                /* NOT budget_reached: this producer stops on ITS OWN step
                 * limit, which is not one of the plan's three budgets, and
                 * headless.rs rejects a budget_reached that did not actually
                 * reach the stated limit. Reporting the real reason keeps a
                 * short run from masquerading as an exhausted budget. */
                emit_hl_end(out, ordinal,
                            "{\"reason\":\"producer_abort\","
                            "\"detail\":\"producer step limit reached\"}",
                            recorded, 0);
            else
                fn64_emit_timing_end(out, ordinal, "completed");
            if (fclose(out) != 0) {
                fprintf(stderr, "closing %s failed\n", out_path);
                _exit(1);
            }
            fn64_append_end_record();
            fprintf(stderr,
                    "trace complete: %llu steps polled, %llu device-event records, "
                    "final ordinal %llu, last Count-derived master-cycle sample %llu\n",
                    recorded, (unsigned long long)(ordinal - 1),
                    (unsigned long long)ordinal, (unsigned long long)cycle);
            pthread_mutex_lock(&g_lock);
            g_done = 1;
            pthread_cond_broadcast(&g_consumed);
            pthread_cond_broadcast(&g_cond);
            pthread_mutex_unlock(&g_lock);
            DebugSetRunState(M64P_DBG_RUNSTATE_RUNNING);
            DebugStep();
            CoreDoCommand(M64CMD_STOP, 0, NULL);
            struct timespec grace = {0, 200 * 1000 * 1000};
            nanosleep(&grace, NULL);
            _exit(0);
        }

        if (fn64_instruction_writes_cp0_count(DebugMemRead32(pc))) {
            if (ordinal != 1) {
                fprintf(stderr,
                        "FATAL: guest instruction at 0x%08x writes CP0 Count after "
                        "device-event emission began; the public debugger exposes no "
                        "independent monotonic master clock. Refusing to continue the "
                        "timing trace across a changed origin.\n",
                        pc);
                if (bundle_sha256)
                    emit_hl_end(out, ordinal,
                                "{\"reason\":\"producer_abort\",\"detail\":"
                                "\"guest wrote cp0 count after first event\"}",
                                recorded, 0);
                else
                    fn64_emit_timing_end(out, ordinal, "aborted");
                fclose(out);
                _exit(3);
            }

            DebugStep();
            wait_for_pause(&pc);
            rebase_count_clock = 1;
            continue;
        }

        DebugStep();
        wait_for_pause(&pc);
    }
}
