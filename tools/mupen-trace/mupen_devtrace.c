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
 *   - {"record":"header","ordinal":0,"schema_version":1,"producer":"...","trace_id":"..."}
 *   - {"record":"device_event","ordinal":N,"event_kind":"dma_start","device":"pi",
 *      "cycle":123,"addr_or_source":32,"value_or_len":64}
 *   - {"record":"end","ordinal":N,"completion":"completed"}
 * `event_kind` in {dma_start, dma_complete, mi_raise, mi_ack, vi_retrace}.
 * `device` in {pi, ai, si, sp, dp, vi, mi} (this producer emits pi/ai/si/vi/mi
 * only -- sp/dp DMA timing is out of this increment's scope per the design
 * spec's "NOT a full mupen integration" boundary).
 * Ordinals are dense: header is 0, then one integer per emitted device_event,
 * then end is next. `ingest_jsonl` in timing_trace.rs rejects gaps.
 *
 * ## Register map (verified against mupen64plus-core source, NOT guessed)
 * mupen addresses PI/SI/AI/MI/VI registers as `(addr & mask) >> 2` into a
 * per-device `regs[]` array (see the headers under `src/device/rcp/`).
 * The KSEG1 (uncached, 0xA4xxxxxx) addresses below were cross-checked against
 * those headers, not assumed from the task brief -- two corrections vs. the
 * commonly-quoted N64dev numbers were caught this way:
 *   - VI_V_INTR_REG is offset 0x0C (index 3), NOT 0x08. VI_WIDTH_REG is 0x08.
 *     (`vi_controller.h`: STATUS=0x00, ORIGIN=0x04, WIDTH=0x08, V_INTR=0x0C,
 *     CURRENT=0x10.)
 *   - AI_STATUS_BUSY is bit 30 (0x40000000) and AI_STATUS_FULL is bit 31
 *     (0x80000000) (`ai_controller.c`), i.e. busy=bit30/full=bit31.
 *   - SI_STATUS: DMA_BUSY = bit0 (0x0001), INTERRUPT = bit12 (0x1000)
 *     (`si_controller.h`).
 *   - MI_INTR source bits: SP=0x01 SI=0x02 AI=0x04 VI=0x08 PI=0x10 DP=0x20
 *     (`mi_controller.h`, `enum mi_intr`) -- identical bit-for-bit to fn64's
 *     `InterruptSource::bit()` (`crates/fn64-runtime/src/device.rs`), so
 *     `addr_or_source` for mi_raise/mi_ack is emitted as this raw mask value,
 *     not a bit index; both producers agree on the encoding by construction.
 *
 * PI_DRAM_ADDR   0xA4600000  PI_CART_ADDR  0xA4600004
 * PI_RD_LEN      0xA4600008  PI_WR_LEN     0xA460000C
 * PI_STATUS      0xA4600010  (bit0 = DMA_BUSY)
 * SI_DRAM_ADDR   0xA4800000
 * SI_STATUS      0xA480001C  (bit0 = DMA_BUSY, bit12 = INTERRUPT)
 * AI_DRAM_ADDR   0xA4500000  AI_LEN        0xA4500004
 * AI_CONTROL     0xA4500008  AI_STATUS     0xA450000C  (bit30=BUSY, bit31=FULL)
 * VI_CURRENT     0xA4400010  VI_V_INTR     0xA440000C  VI_V_SYNC 0xA4400018
 * MI_INTR        0xA4300008  MI_INTR_MASK  0xA430000C
 *
 * ## Guest cycle
 * `DebugGetCPUDataPtr(M64P_CPU_REG_COP0)` returns a pointer to the live
 * `uint32_t cop0[32]` register file; index 9 is CP0 Count
 * (`enum r4300_cp0_registers` in `src/device/r4300/cp0.h`: ..., BADVADDR=8,
 * COUNT=9, ENTRYHI=10, ...). This is the real hardware guest-cycle counter
 * (Count increments once every 2 CPU cycles per the R4300 manual; mupen
 * exposes it directly, not a step index), read fresh at every pause -- no
 * fallback to step-index is needed since this pointer is always available on
 * a DEBUGGER=1 build (verified: `DebugGetCPUDataPtr` is an exported symbol on
 * Jer's arm64 core). The `cycle` field in every emitted record is this raw
 * Count value, so it is directly comparable to fn64's `DeviceFabric` cycle
 * stamps (both count R4300 guest cycles, not wall time or instruction count).
 *
 * ## Detection strategy: per-step register polling, not write interception
 * Like `mupen_trace.c`'s watched-cell poller, this producer re-reads the
 * MMIO registers after every retired instruction (via DebugMemRead32) and
 * emits a record on every observed EDGE:
 *   - PI/SI: DMA_BUSY 0->1 is `dma_start` (addr_or_source = CART_ADDR for PI,
 *     DRAM_ADDR for SI since SI has no cart-address register; value_or_len =
 *     WR_LEN+1/RD_LEN+1 for PI, 0 for SI -- SI's PIF window is a fixed 64
 *     bytes and mupen carries no explicit length register for it, mirroring
 *     the fn64-side tap's own `value_or_len: 0` convention for SI in
 *     timing_trace.rs). DMA_BUSY 1->0 is `dma_complete` with the same payload
 *     captured at start (mupen clears PI_WR_LEN/RD_LEN back to 0x7F as a
 *     read-only quirk -- see `read_pi_regs` -- so the complete record reuses
 *     the start payload rather than re-reading a now-meaningless length).
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
 *   - VI: `vi_retrace` on a VI_CURRENT wrap, gated on VI_CURRENT dropping
 *     below its previous value (a genuine field-boundary wrap, not per-step
 *     noise -- VI_CURRENT only changes on its own hardware clock, not every
 *     R4300 step, so any decrease is a wrap by construction).
 * Because register state is only visible at pause boundaries, an edge is
 * detected at most one instruction late (i.e. at the cycle count of the
 * FIRST step where the effect is observable), matching the same latency
 * `mupen_trace.c` already accepts for its watched-cell polling.
 *
 * ## Recording window
 * Unlike `mupen_trace.c`, this producer does NOT gate recording on reaching
 * the NW4E resident entrypoint (0x80000400): device timing during IPL3/boot
 * (the very first PI DMAs that copy the resident image off the cartridge)
 * is exactly the kind of event this oracle exists to compare, so recording
 * starts at the very first debugger pause and runs for `steps` retired
 * instructions.
 *
 * Build (macOS, Homebrew mupen64plus headers):
 *   cc -O2 -Wall -Wextra -o mupen_devtrace mupen_devtrace.c \
 *      -I/opt/homebrew/Cellar/mupen64plus/2.6.0/include -lpthread
 * Run:
 *   ./mupen_devtrace <core.dylib> <rom.z64> <rsp.dylib> <out.jsonl> <steps> <trace_id>
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <dlfcn.h>
#include <pthread.h>
#include <unistd.h>
#include <time.h>

/* Public m64p API headers only -- no core implementation source linked. */
#include <mupen64plus/m64p_types.h>
#include <mupen64plus/m64p_common.h>
#include <mupen64plus/m64p_config.h>
#include <mupen64plus/m64p_frontend.h>
#include <mupen64plus/m64p_debugger.h>

/* ---- verified RCP register addresses (KSEG1, uncached); see header ---- */
#define PI_DRAM_ADDR 0xA4600000u
#define PI_CART_ADDR 0xA4600004u
#define PI_RD_LEN    0xA4600008u
#define PI_WR_LEN    0xA460000Cu
#define PI_STATUS    0xA4600010u
#define PI_STATUS_DMA_BUSY 0x01u

#define SI_DRAM_ADDR 0xA4800000u
#define SI_STATUS    0xA480001Cu
#define SI_STATUS_DMA_BUSY 0x0001u

#define AI_DRAM_ADDR 0xA4500000u
#define AI_LEN       0xA4500004u
#define AI_STATUS    0xA450000Cu
#define AI_STATUS_BUSY 0x40000000u

#define VI_CURRENT   0xA4400010u

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

static void dbg_init(void) { g_dbg_ready = 1; }
static void dbg_vi(void) {}

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

/* ---- JSONL emission, matching timing_trace.rs's DeviceTraceRecord exactly
 * (serde tag = "record", rename_all = "snake_case" on every enum). ---- */

static void emit_header(FILE *out, const char *producer, const char *trace_id) {
    fprintf(out,
            "{\"record\":\"header\",\"ordinal\":0,\"schema_version\":1,"
            "\"producer\":\"%s\",\"trace_id\":\"%s\"}\n",
            producer, trace_id);
}

static void emit_event(FILE *out, uint64_t ordinal, const char *event_kind,
                        const char *device, uint64_t cycle, uint32_t addr_or_source,
                        uint32_t value_or_len) {
    fprintf(out,
            "{\"record\":\"device_event\",\"ordinal\":%llu,\"event_kind\":\"%s\","
            "\"device\":\"%s\",\"cycle\":%llu,\"addr_or_source\":%u,"
            "\"value_or_len\":%u}\n",
            (unsigned long long)ordinal, event_kind, device,
            (unsigned long long)cycle, addr_or_source, value_or_len);
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

static void emit_end(FILE *out, uint64_t ordinal, const char *completion) {
    fprintf(out, "{\"record\":\"end\",\"ordinal\":%llu,\"completion\":\"%s\"}\n",
            (unsigned long long)ordinal, completion);
}

/* ---- per-device previous-poll state, for edge detection ---- */
struct pi_state {
    int busy;
    uint32_t cart_addr;
    /* The timing schema had no room for this; the headless schema needs it,
     * and it must be captured on the SAME poll as cart_addr and len. */
    uint32_t dram_addr;
    uint32_t len;
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
    if (steps == 0) {
        fprintf(stderr, "steps must be > 0\n");
        return 2;
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
    m64p_handle core_section = NULL;
    int one = 1;
    int interpreter = 0; /* pure interpreter: deterministic, debuggable */
    if (ConfigOpenSection("Core", &core_section) != M64ERR_SUCCESS ||
        ConfigSetParameter(core_section, "EnableDebugger", M64TYPE_BOOL, &one) != M64ERR_SUCCESS ||
        ConfigSetParameter(core_section, "R4300Emulator", M64TYPE_INT, &interpreter) !=
            M64ERR_SUCCESS) {
        fprintf(stderr, "failed to set [Core] EnableDebugger/R4300Emulator\n");
        return 1;
    }

    /* Arm the debugger BEFORE ROM open (documented ordering requirement). */
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
             "mupen-devtrace v1 (mupen64plus-core DEBUGGER=1 pure-interpreter + rsp plugin, "
             "single-step device-register polling via public m64p_debugger API)");
    if (bundle_sha256)
        emit_hl_header(out, trace_id, bundle_sha256);
    else
        emit_header(out, producer, trace_id);
    uint64_t ordinal = 1;

    /* First pause establishes the CP0 register file pointer (stable for the
     * process lifetime once the debugger is armed) and baselines every
     * device's previous-poll state so the very first step cannot manufacture
     * a spurious edge against zeroed C statics. */
    uint32_t pc;
    wait_for_pause(&pc);

    uint32_t *cop0 = (uint32_t *)DebugGetCPUDataPtr(M64P_CPU_REG_COP0);
    if (!cop0) {
        fprintf(stderr, "DebugGetCPUDataPtr(COP0) returned NULL\n");
        _exit(1);
    }
#define GUEST_CYCLE ((uint64_t)cop0[9]) /* CP0_COUNT_REG, see cp0.h */

    struct pi_state pi_prev;
    struct si_state si_prev;
    struct ai_state ai_prev;
    pi_prev.busy = DebugMemRead32(PI_STATUS) & PI_STATUS_DMA_BUSY;
    pi_prev.cart_addr = DebugMemRead32(PI_CART_ADDR);
    pi_prev.dram_addr = DebugMemRead32(PI_DRAM_ADDR);
    pi_prev.len = 0;
    si_prev.busy = DebugMemRead32(SI_STATUS) & SI_STATUS_DMA_BUSY;
    si_prev.dram_addr = DebugMemRead32(SI_DRAM_ADDR);
    ai_prev.busy = (DebugMemRead32(AI_STATUS) & AI_STATUS_BUSY) != 0;
    ai_prev.dram_addr = DebugMemRead32(AI_DRAM_ADDR);
    ai_prev.len = DebugMemRead32(AI_LEN);
    uint32_t mi_prev = DebugMemRead32(MI_INTR);
    uint32_t vi_prev = DebugMemRead32(VI_CURRENT);

    unsigned long long recorded = 0;
    for (;;) {
        uint64_t cycle = GUEST_CYCLE;

        /* ---- PI DMA edges ---- */
        uint32_t pi_status = DebugMemRead32(PI_STATUS);
        int pi_busy = (pi_status & PI_STATUS_DMA_BUSY) != 0;
        if (pi_busy && !pi_prev.busy) {
            /* Rising edge: WR_LEN/RD_LEN were just consumed by the write
             * that started the DMA (mupen's read_pi_regs clamps them back to
             * 0x7F immediately, a quirk the header documents), so capture
             * CART_ADDR now and derive length from the RD/WR_LEN value the
             * SAME poll observes -- both registers reflect the just-started
             * transfer's parameters at this exact pause since the register
             * write and dma_pi_read/write() are atomic within one retired
             * store instruction. */
            uint32_t rd_len = DebugMemRead32(PI_RD_LEN);
            uint32_t wr_len = DebugMemRead32(PI_WR_LEN);
            uint32_t len = (rd_len != 0x7F) ? (rd_len + 1) : ((wr_len != 0x7F) ? (wr_len + 1) : 0);
            pi_prev.cart_addr = DebugMemRead32(PI_CART_ADDR);
            pi_prev.dram_addr = DebugMemRead32(PI_DRAM_ADDR);
            pi_prev.len = len;
            if (!bundle_sha256)
                emit_event(out, ordinal++, "dma_start", "pi", cycle, pi_prev.cart_addr, len);
        } else if (!pi_busy && pi_prev.busy) {
            if (bundle_sha256) {
                /* Falling edge only. headless.rs names this record
                 * PiDmaCompleted deliberately: a register write or a
                 * DMA-start notification is explicitly NOT sufficient
                 * evidence, because a started transfer need not complete
                 * with the geometry it started with. */
                if (pi_prev.len)
                    emit_hl_pi_dma(out, ordinal++, "pi_dma_loads", pi_prev.cart_addr,
                                   pi_prev.dram_addr, pi_prev.len);
            } else {
                emit_event(out, ordinal++, "dma_complete", "pi", cycle, pi_prev.cart_addr,
                           pi_prev.len);
            }
        }
        pi_prev.busy = pi_busy;

        /* ---- SI DMA edges (fixed 64-byte PIF window; no length register,
         * mirroring the fn64-side tap's value_or_len=0 convention for SI) ---- */
        uint32_t si_status = DebugMemRead32(SI_STATUS);
        int si_busy = (si_status & SI_STATUS_DMA_BUSY) != 0;
        if (si_busy && !si_prev.busy) {
            si_prev.dram_addr = DebugMemRead32(SI_DRAM_ADDR);
            emit_event(out, ordinal++, "dma_start", "si", cycle, si_prev.dram_addr, 0);
        } else if (!si_busy && si_prev.busy) {
            emit_event(out, ordinal++, "dma_complete", "si", cycle, si_prev.dram_addr, 0);
        }
        si_prev.busy = si_busy;

        /* ---- AI DMA edges (2-deep FIFO caveat: see header comment) ---- */
        uint32_t ai_status = DebugMemRead32(AI_STATUS);
        int ai_busy = (ai_status & AI_STATUS_BUSY) != 0;
        if (ai_busy && !ai_prev.busy) {
            ai_prev.dram_addr = DebugMemRead32(AI_DRAM_ADDR);
            ai_prev.len = DebugMemRead32(AI_LEN);
            emit_event(out, ordinal++, "dma_start", "ai", cycle, ai_prev.dram_addr, ai_prev.len);
        } else if (!ai_busy && ai_prev.busy) {
            emit_event(out, ordinal++, "dma_complete", "ai", cycle, ai_prev.dram_addr, ai_prev.len);
        }
        ai_prev.busy = ai_busy;

        /* ---- MI interrupt raise/ack: bit-for-bit diff ---- */
        uint32_t mi_now = DebugMemRead32(MI_INTR);
        uint32_t raised = mi_now & ~mi_prev;
        uint32_t acked = mi_prev & ~mi_now;
        for (uint32_t bit = 0x01; bit <= 0x20; bit <<= 1) {
            if (raised & bit)
                emit_event(out, ordinal++, "mi_raise", "mi", cycle, bit, 0);
            if (acked & bit)
                emit_event(out, ordinal++, "mi_ack", "mi", cycle, bit, 0);
        }
        mi_prev = mi_now;

        /* ---- VI retrace: VI_CURRENT wraps (decreases) on a field
         * boundary; it never decreases for any other reason since it only
         * moves on VI's own clock, not per R4300 step. ---- */
        uint32_t vi_now = DebugMemRead32(VI_CURRENT);
        if (vi_now < vi_prev)
            emit_event(out, ordinal++, "vi_retrace", "vi", cycle, 0, 0);
        vi_prev = vi_now;

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
                emit_end(out, ordinal, "completed");
            if (fclose(out) != 0) {
                fprintf(stderr, "closing %s failed\n", out_path);
                _exit(1);
            }
            fprintf(stderr,
                    "trace complete: %llu steps polled, %llu device-event records, "
                    "final ordinal %llu, last guest cycle %llu\n",
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

        DebugStep();
        wait_for_pause(&pc);
    }
}
