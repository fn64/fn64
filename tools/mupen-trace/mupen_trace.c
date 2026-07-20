/*
 * mupen_trace.c -- fn64 trace producer v1 (bounded NW4E boot trace).
 *
 * Drives a DEBUGGER=1 mupen64plus-core build purely through its PUBLIC,
 * documented frontend API (m64p_common.h, m64p_frontend.h, m64p_debugger.h,
 * m64p_types.h) via dlopen/dlsym. No core implementation source was read
 * beyond those public API headers (doc comments + prototypes only); this
 * file descends from the prior session's watch.c poller and replaces the
 * wall-clock poll loop with the documented single-step mechanism
 * (M64P_DBG_RUNSTATE_STEPPING + DebugStep + the DebugSetCallbacks update
 * callback, which reports the PC at every pause).
 *
 * Output: fn64-discover trace-schema JSONL (crates/fn64-discover/src/trace.rs,
 * TRACE_SCHEMA_VERSION 1):
 *   - sequence 0: header bound to the normalized ROM SHA-256. The input must
 *     be native big-endian (.z64, magic 80 37 12 40) so the normalized bytes
 *     ARE the file bytes; anything else fails loudly rather than byteswapping.
 *   - executed_pc records over a BOUNDED window: recording starts the first
 *     time the debugger pauses at the ROM header entrypoint (0x80000400 for
 *     NW4E) and covers exactly `steps` executed instructions. A PC is only
 *     emitted once the NEXT pause proves the instruction actually retired
 *     (the update callback reports the address about to execute).
 *   - watched_table_write records: after every retired instruction the two
 *     watched cells (selector flag word 0x800a10b0, mode byte 0x80097fd8)
 *     are re-read via DebugMemRead32/8; every VALUE TRANSITION is emitted.
 *     These are observed-value records at instruction granularity -- they
 *     carry no write-PC attribution and a store that rewrites the same value
 *     is invisible, which is why the end record claims bounded
 *     exhaustiveness for executed_pc ONLY, never for the watch domains.
 *   - end record: completion=completed with one bounded executed_pc
 *     exhaustiveness claim spanning the recorded window. Execution before
 *     the entrypoint pause (IPL3/boot-stub churn) is intentionally outside
 *     the claimed interval and is not recorded at all.
 *
 * Bank identity: a PC inside [0x80000400, 0x80056670) is attributed to the
 * always-resident "boot" bank (activation 0). Justification from byte-
 * verified boot-copy facts (fn64-discover Phase 2 + the NW4E descriptor
 * table at ROM 0x539a0): IPL3 copies the resident image from ROM 0x1000 to
 * the header entrypoint VA 0x80000400, and every overlay bank's load
 * destination (0x800d9960 slot A / 0x80106760 slot B) lies strictly above
 * this window, so no overlay load can ever alias it. Every other address
 * (IPL3 stub, RSP vector space, data regions) stays bank-unknown --
 * unknown-preserved, never guessed.
 *
 * Determinism: the trace contains sequence numbers only, never timestamps.
 * The interpreter core plus rsp-hle plus dummy video/audio/input plugins is
 * instruction-deterministic, so repeated captures must be byte-identical
 * (the task bar: 3+ identical runs before the trace is called captured).
 *
 * Build (macOS, Homebrew mupen64plus headers; CommonCrypto for SHA-256):
 *   cc -O2 -Wall -Wextra -o mupen_trace mupen_trace.c \
 *      -I/opt/homebrew/Cellar/mupen64plus/2.6.0/include -lpthread
 * Run:
 *   ./mupen_trace <core.dylib> <rom.z64> <rsp.dylib> <out.jsonl> <steps> <trace_id>
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <dlfcn.h>
#include <pthread.h>
#include <unistd.h>

#include <CommonCrypto/CommonDigest.h>

/* Public m64p API headers only (enums + ptr_* typedefs; no
 * M64P_CORE_PROTOTYPES, no linkage against the core). */
#include <mupen64plus/m64p_types.h>
#include <mupen64plus/m64p_common.h>
#include <mupen64plus/m64p_config.h>
#include <mupen64plus/m64p_frontend.h>
#include <mupen64plus/m64p_debugger.h>

#define ADDR_FLAG 0x800a10b0u
#define ADDR_MODE 0x80097fd8u

/* Resident-bank attribution window; see the header comment for the
 * boot-copy justification. */
#define RESIDENT_VA_START 0x80000400u
#define RESIDENT_VA_END 0x80056670u

#define RECORD_START_PC 0x80000400u /* NW4E header entrypoint */

/* Give up if the entrypoint pause never arrives within this many pre-window
 * steps (loud failure, not a hang). */
#define MAX_SKIP_STEPS 40000000ull

static ptr_DebugMemRead32 DebugMemRead32;
static ptr_DebugMemRead8 DebugMemRead8;
static ptr_DebugSetRunState DebugSetRunState;
static ptr_DebugStep DebugStep;
static ptr_CoreDoCommand g_do_command;

/* ---- update-callback -> main-thread handshake ----
 * The update callback runs on the emulation thread each time the debugger
 * pauses (per the m64p_debugger.h doc comments). It hands the PC to the
 * main thread and returns immediately; the main thread records/reads
 * memory while the core is paused, then calls DebugStep() to retire one
 * more instruction. */
static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t g_cond = PTHREAD_COND_INITIALIZER;
static pthread_cond_t g_consumed = PTHREAD_COND_INITIALIZER;
static uint32_t g_pc;
static int g_have_pc;
static volatile int g_done;

static volatile int g_dbg_ready;

/* Called by the core when the debugger is initialized (m64p_debugger.h:
 * DebugSetCallbacks's first callback). Only after this point is a run-state
 * change meaningful, so the main thread gates its STEPPING transition on it. */
static void dbg_init(void) { g_dbg_ready = 1; }
static void dbg_vi(void) {}

/* Blocks until the main thread consumed the previous report, so a pause
 * report can never be overwritten -- the executed_pc stream must not have
 * silent gaps or the bounded exhaustiveness claim would be a lie. */
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

/* Wait for the next pause report. The public API does not document whether
 * the core reports a pause before or after it starts waiting for the step
 * signal, so a bounded timed wait re-kicks DebugStep() when no report
 * arrives; a kick while the core is mid-instruction is absorbed by the
 * core's own signaling and cannot skip an instruction report. Kicks are
 * host-side scheduling only and never appear in the trace. */
static uint64_t g_kicks;

static int wait_for_pause(uint32_t *pc_out) {
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
    return 0;
}

static int pc_in_resident(uint32_t pc) {
    return pc >= RESIDENT_VA_START && pc < RESIDENT_VA_END;
}

static void emit_executed_pc(FILE *out, uint64_t seq, uint32_t pc) {
    if (pc_in_resident(pc)) {
        fprintf(out,
                "{\"event\":\"executed_pc\",\"sequence\":%llu,\"pc\":{\"address\":%u,"
                "\"bank\":{\"status\":\"known\",\"bank\":\"boot\",\"activation\":0}}}\n",
                (unsigned long long)seq, pc);
    } else {
        fprintf(out,
                "{\"event\":\"executed_pc\",\"sequence\":%llu,\"pc\":{\"address\":%u,"
                "\"bank\":{\"status\":\"unknown\"}}}\n",
                (unsigned long long)seq, pc);
    }
}

static void emit_watch(FILE *out, uint64_t seq, const char *watch_id, uint32_t address,
                       const char *width, uint64_t value) {
    /* The writer's bank is unobserved: these are polled value transitions,
     * so active_bank stays unknown-preserved. */
    fprintf(out,
            "{\"event\":\"watched_table_write\",\"sequence\":%llu,\"watch_id\":\"%s\","
            "\"address\":%u,\"width\":\"%s\",\"value\":%llu,"
            "\"active_bank\":{\"status\":\"unknown\"}}\n",
            (unsigned long long)seq, watch_id, address, width, (unsigned long long)value);
}

int main(int argc, char **argv) {
    if (argc != 7) {
        fprintf(stderr,
                "usage: %s <core.dylib> <rom.z64> <rsp.dylib> <out.jsonl> <steps> <trace_id>\n",
                argv[0]);
        return 2;
    }
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

    /* ---- ROM bytes + normalized digest (z64 is already big-endian) ---- */
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
    unsigned char digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256(rombuf, (CC_LONG)romlen, digest);
    char digest_hex[CC_SHA256_DIGEST_LENGTH * 2 + 1];
    for (int i = 0; i < CC_SHA256_DIGEST_LENGTH; i++)
        snprintf(digest_hex + i * 2, 3, "%02x", digest[i]);

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
    DebugMemRead8 = (ptr_DebugMemRead8)dlsym(core, "DebugMemRead8");
    if (!CoreStartup || !CoreAttachPlugin || !CoreDoCommand || !DebugSetCallbacks ||
        !DebugSetRunState || !DebugStep || !DebugMemRead32 || !DebugMemRead8) {
        fprintf(stderr, "missing required symbol(s) -- was this core built with DEBUGGER=1?\n");
        return 1;
    }

    m64p_error rc = CoreStartup(0x020001, NULL, NULL, NULL, debug_cb, NULL, state_cb);
    if (rc != M64ERR_SUCCESS) {
        fprintf(stderr, "CoreStartup -> %d (%s)\n", rc,
                CoreErrorMessage ? CoreErrorMessage(rc) : "?");
        return 1;
    }
    /* The core only activates the debugger when the documented [Core]
     * config parameter `EnableDebugger` is set ("Activate the R4300
     * debugger when ROM execution begins, if core was built with Debugger
     * support" -- the core's own generated config comment). Set it
     * in-process, plus `R4300Emulator = 0` (pure interpreter) so the
     * capture never depends on the user's on-disk config. Nothing is saved
     * back to disk. */
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
    int interpreter = 0;
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
    /* Core-internal dummy gfx/audio/input plugins. */
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

    /* The debugger initializes inside EXECUTE and starts paused; a run-state
     * change issued before its init callback is ignored or reset. Wait for
     * the documented init callback, then enter stepping mode and kick the
     * first step. */
    for (int waited_ms = 0; !g_dbg_ready; waited_ms += 10) {
        if (waited_ms > 20000) {
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

    fprintf(out,
            "{\"event\":\"header\",\"sequence\":0,\"schema_version\":1,"
            "\"normalized_rom_sha256\":\"%s\",\"trace_id\":\"%s\","
            "\"producer\":\"mupen-trace v1 (mupen64plus-core 2.6.0 DEBUGGER=1 b0d68c2 "
            "pure-interpreter + rsp-hle, single-step via public m64p_debugger API)\"}\n",
            digest_hex, trace_id);

    uint64_t seq = 1;
    uint64_t skip_steps = 0;
    unsigned long long recorded = 0;
    int recording = 0;
    int have_prev = 0;
    uint32_t prev_pc = 0;
    uint32_t last_flag = 0;
    uint32_t last_mode = 0;
    uint64_t first_claim_seq = 0;

    for (;;) {
        uint32_t pc;
        wait_for_pause(&pc);

        if (!recording) {
            if (pc == RECORD_START_PC) {
                recording = 1;
                /* Baseline observed values at the entrypoint pause. */
                last_flag = DebugMemRead32(ADDR_FLAG);
                last_mode = DebugMemRead8(ADDR_MODE);
                emit_watch(out, seq++, "selector-flag-0x800a10b0", ADDR_FLAG, "u32", last_flag);
                emit_watch(out, seq++, "mode-byte-0x80097fd8", ADDR_MODE, "u8", last_mode);
                first_claim_seq = seq;
                prev_pc = pc;
                have_prev = 1;
                fprintf(stderr, "entrypoint pause at 0x%08x after %llu pre-window steps\n", pc,
                        (unsigned long long)skip_steps);
            } else if (++skip_steps > MAX_SKIP_STEPS) {
                fprintf(stderr,
                        "entrypoint 0x%08x never reached within %llu steps; aborting\n",
                        RECORD_START_PC, (unsigned long long)MAX_SKIP_STEPS);
                fprintf(out,
                        "{\"event\":\"end\",\"sequence\":%llu,\"completion\":\"aborted\","
                        "\"exhaustiveness\":[]}\n",
                        (unsigned long long)seq);
                fclose(out);
                g_done = 1;
                _exit(1);
            }
            DebugStep();
            continue;
        }

        /* This pause proves the instruction at prev_pc retired. */
        if (have_prev) {
            emit_executed_pc(out, seq++, prev_pc);
            recorded++;
            uint32_t flag = DebugMemRead32(ADDR_FLAG);
            uint32_t mode = DebugMemRead8(ADDR_MODE);
            if (flag != last_flag) {
                emit_watch(out, seq++, "selector-flag-0x800a10b0", ADDR_FLAG, "u32", flag);
                last_flag = flag;
            }
            if (mode != last_mode) {
                emit_watch(out, seq++, "mode-byte-0x80097fd8", ADDR_MODE, "u8", mode);
                last_mode = mode;
            }
        }
        prev_pc = pc;

        if (recorded >= steps) {
            uint64_t last_claim_seq = seq - 1;
            fprintf(out,
                    "{\"event\":\"end\",\"sequence\":%llu,\"completion\":\"completed\","
                    "\"exhaustiveness\":[{\"domain\":\"executed_pc\","
                    "\"first_sequence\":%llu,\"last_sequence\":%llu}]}\n",
                    (unsigned long long)seq, (unsigned long long)first_claim_seq,
                    (unsigned long long)last_claim_seq);
            if (fclose(out) != 0) {
                fprintf(stderr, "closing %s failed\n", out_path);
                _exit(1);
            }
            fprintf(stderr, "trace complete: %llu executed-pc records, final sequence %llu\n",
                    recorded, (unsigned long long)seq);
            /* Teardown: the core is paused inside its own stepping wait on
             * the EXECUTE thread. The documented shutdown (RUNNING + STOP)
             * is attempted; if the exec thread does not come back promptly
             * the process exits anyway -- the trace file is already closed
             * and complete, and this exit path touches no output. */
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
    }
}
