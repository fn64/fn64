/*
 * mupen_bfs.c -- fn64 breakpoint-accelerated NW4E selector-dispatcher
 * state explorer.
 *
 * Descends directly from tools/mupen-trace/mupen_trace.c (same core, same
 * dlopen/dlsym public-API discipline, same update-callback pause handshake).
 * mupen_trace.c drives the core by single-stepping every retired
 * instruction and logging every one, which reaches only ~500k instructions
 * into boot before its step/output budget runs out -- short of the
 * selector dispatcher's thread-created entry point.
 *
 * IMPORTANT MEASURED FINDING (see the driver's dev-session notes; not
 * assumed, timed): on this DEBUGGER=1 core build, M64P_DBG_RUNSTATE_RUNNING
 * does NOT run the interpreter freely between breakpoint hits the way the
 * public API's naming implies. `sample`-confirmed backtraces show
 * update_debugger() calls the update callback and blocks on a
 * SDL_SemWaitTimeout on EVERY retired instruction whenever the debugger is
 * armed, identically under RUNNING and STEPPING -- the only way to advance
 * past that wait, in either run state, is DebugStep(). Worse, timed A/B
 * comparison (mupen_bfs_timing2/5 in the dev session; not preserved here,
 * numbers below) showed RUNNING+DebugStep() round trips are catastrophically
 * slower than STEPPING+DebugStep() round trips with the SAME breakpoints
 * installed -- RUNNING did not clear 50,000 resumes in 25 wall-clock
 * seconds (~28% CPU, i.e. genuinely computing, not blocked) while
 * STEPPING did 300,000 resumes with real forward PC progress in a few
 * seconds, matching mupen_trace.c's plain-STEPPING, no-breakpoints baseline
 * rate of ~216,000 steps/sec (5,077,962 steps in 23.6s measured directly).
 * This driver therefore uses STEPPING throughout, exactly like
 * mupen_trace.c, and the acceleration over mupen_trace.c comes from a
 * DIFFERENT mechanism than originally planned: not "run free to a
 * breakpoint," but skipping mupen_trace.c's per-instruction JSONL
 * formatting/write and its bounded-window recording discipline, plus using
 * the core's own breakpoint-triggered-by query so the driver does not need
 * to branch on PC every step, and a write watchpoint on the flag word so
 * overlay-bank stores (which only exist once that overlay's bytes are
 * loaded into RDRAM, so they cannot be breakpointed by PC before load
 * time) are still caught without a per-instruction memory poll.
 *
 * Breakpoints installed (see aki_reference::NW4E_SELECTOR /
 * gate_selector.rs, this repo's byte-verified NW4E answer key):
 *   EXEC  0x800268f0  dispatcher init store  (sw $zero -> flag; PROVES the
 *                                             dispatcher itself started)
 *   EXEC  0x8002693c  r2 test load   (lw flag, right after r2_test_pc lui)
 *   EXEC  0x800269d4  r3 test load
 *   EXEC  0x80026a68  r5 test load
 *   EXEC  0x80026b00  loop test load
 *   WRITE 0x800a10b0  flag word itself, 4 bytes -- catches every store to
 *                     the flag from ANY bank (resident init + all overlay
 *                     R2/R3/R5 stores), regardless of whether that overlay
 *                     bank's PCs were separately breakpointed. This is the
 *                     mechanism that reaches the overlay stores at all:
 *                     0x80106824/0x80106940/0x80106dac/0x80106dec/
 *                     0x80109124/0x80109140/0x80109178 only exist in RDRAM
 *                     after their owning overlay is DMA'd in, so an EXEC
 *                     breakpoint at those fixed VAs cannot be relied on
 *                     before that point -- the write watchpoint has no such
 *                     requirement, it fires on the address regardless of
 *                     which bank currently occupies it.
 *
 * On every pause this driver reads:
 *   - the current PC (DebugGetCPUDataPtr(M64P_CPU_PC))
 *   - which breakpoint(s) triggered (DebugBreakpointTriggeredBy)
 *   - the flag word (DebugMemRead32 0x800a10b0) and mode byte
 *     (DebugMemRead8 0x80097fd8)
 * and, only for a NAMED hit (a real breakpoint match) or the initial
 * baseline pause, appends one JSON-lines record; then resumes STEPPING.
 *
 * Determinism: same pure-interpreter, same dummy gfx/audio/input plugins,
 * no timestamps in the record stream -- only sequence numbers -- so repeat
 * runs must be byte-identical. Observed: 4 consecutive runs to max_hits=2
 * (baseline + init_store + r2_load) produced byte-identical output
 * (MD5 fd80d8c3f364b5842cba24e2be5f7c56 all four times); the task's 10x bar
 * was not completed in-session because each run's wall-clock time to reach
 * even hit #2 varies widely (observed 1s to 160s) under this build's real
 * VI/PI-timing-paced boot waits, and a longer run chasing hit #3 (R3/R5)
 * ran 114M+ single-stepped instructions over 10 minutes without reaching
 * it -- see the R2/R3/R5-not-reached note in the report this driver
 * shipped with.
 *
 * Clean room: public m64p_debugger.h/m64p_types.h/m64p_common.h/
 * m64p_frontend.h API only, via dlopen/dlsym. No core implementation source
 * read for this file.
 *
 * Build (macOS, Homebrew mupen64plus headers; CommonCrypto for SHA-256):
 *   cc -O2 -Wall -Wextra -o mupen_bfs mupen_bfs.c \
 *      -I/opt/homebrew/Cellar/mupen64plus/2.6.0/include -lpthread
 * Run:
 *   ./mupen_bfs <core.dylib> <rom.z64> <rsp.dylib> <out.jsonl> <max_hits> <budget_steps>
 *
 * <max_hits>: stop after this many breakpoint pause records (0 = unbounded,
 * governed only by budget_steps / wall clock).
 * <budget_steps>: safety ceiling on RUNNING-resume cycles issued before
 * giving up and reporting "aborted" (loud failure, not a silent hang) --
 * this bounds pathological cases where the run never reaches a breakpoint
 * (e.g. wrong PCs), not normal operation, since each resume typically runs
 * millions of guest instructions before the next hit.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <dlfcn.h>
#include <pthread.h>
#include <unistd.h>
#include <time.h>

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

#define RECORD_START_PC 0x80000400u /* NW4E header entrypoint */

/* How often (in resume/step count) to print a liveness heartbeat while
 * stepping between named hits. Purely diagnostic; never appears in the
 * JSONL output, so it cannot affect determinism of the recorded trace. */
#define HEARTBEAT_RESUMES 200000ull

/* Byte-verified dispatcher PCs, aki_reference::NW4E_SELECTOR /
 * gate_selector.rs. */
#define BP_INIT_STORE 0x800268f0u
#define BP_R2_LOAD 0x8002693cu
#define BP_R3_LOAD 0x800269d4u
#define BP_R5_LOAD 0x80026a68u
#define BP_LOOP_LOAD 0x80026b00u

#define NUM_EXEC_BPS 5
static const uint32_t EXEC_BPS[NUM_EXEC_BPS] = {
    BP_INIT_STORE, BP_R2_LOAD, BP_R3_LOAD, BP_R5_LOAD, BP_LOOP_LOAD,
};
static const char *EXEC_BP_NAMES[NUM_EXEC_BPS] = {
    "init_store", "r2_load", "r3_load", "r5_load", "loop_load",
};

static ptr_DebugMemRead32 DebugMemRead32;
static ptr_DebugMemRead8 DebugMemRead8;
static ptr_DebugSetRunState DebugSetRunState;
static ptr_DebugStep DebugStep;
static ptr_DebugGetState DebugGetState;
static ptr_DebugGetCPUDataPtr DebugGetCPUDataPtr;
static ptr_DebugBreakpointCommand DebugBreakpointCommand;
static ptr_DebugBreakpointTriggeredBy DebugBreakpointTriggeredBy;
static ptr_CoreDoCommand g_do_command;

/* ---- update-callback -> main-thread handshake (same pattern as
 * mupen_trace.c: the callback runs on the emulation thread each time the
 * debugger pauses, hands the PC to the main thread, and blocks until the
 * main thread has consumed it so no pause report can be silently
 * overwritten). */
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

/* Same bounded timed-wait + re-kick pattern as mupen_trace.c: the public
 * API does not document whether a pause report is posted before or after
 * the core starts waiting, so a periodic re-issue of the current run
 * command (RUNNING) is a host-side scheduling nudge only, absorbed by the
 * core's own signaling, and never appears in the recorded trace. */
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
            /* update_debugger()'s internal wait (mupen_trace.c's
             * precedent, confirmed here by `sample` backtrace during
             * development) is released only by DebugStep(); a kick while
             * the core is already mid-instruction is absorbed by the
             * core's own signaling and cannot skip or duplicate a
             * breakpoint report. */
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

/* Resume from a paused state via single-step (see the file header's
 * MEASURED FINDING: RUNNING is not faster than STEPPING on this core once
 * any breakpoint is installed, and only STEPPING was verified fast). The
 * name is kept as `resume_running` for call-site continuity even though it
 * issues STEPPING -- callers only need "make forward progress." */
static void resume_running(void) {
    DebugSetRunState(M64P_DBG_RUNSTATE_STEPPING);
    DebugStep();
}

static const char *exec_bp_name(uint32_t pc) {
    for (int i = 0; i < NUM_EXEC_BPS; i++)
        if (EXEC_BPS[i] == pc)
            return EXEC_BP_NAMES[i];
    return NULL;
}

int main(int argc, char **argv) {
    if (argc != 7) {
        fprintf(stderr,
                "usage: %s <core.dylib> <rom.z64> <rsp.dylib> <out.jsonl> <max_hits> <budget_resumes>\n",
                argv[0]);
        return 2;
    }
    const char *core_path = argv[1];
    const char *rom_path = argv[2];
    const char *rsp_path = argv[3];
    const char *out_path = argv[4];
    unsigned long long max_hits = strtoull(argv[5], NULL, 10);
    unsigned long long budget_resumes = strtoull(argv[6], NULL, 10);
    if (budget_resumes == 0) {
        fprintf(stderr, "budget_resumes must be > 0\n");
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
    DebugGetState = (ptr_DebugGetState)dlsym(core, "DebugGetState");
    DebugMemRead32 = (ptr_DebugMemRead32)dlsym(core, "DebugMemRead32");
    DebugMemRead8 = (ptr_DebugMemRead8)dlsym(core, "DebugMemRead8");
    DebugGetCPUDataPtr = (ptr_DebugGetCPUDataPtr)dlsym(core, "DebugGetCPUDataPtr");
    DebugBreakpointCommand = (ptr_DebugBreakpointCommand)dlsym(core, "DebugBreakpointCommand");
    DebugBreakpointTriggeredBy =
        (ptr_DebugBreakpointTriggeredBy)dlsym(core, "DebugBreakpointTriggeredBy");
    if (!CoreStartup || !CoreAttachPlugin || !CoreDoCommand || !DebugSetCallbacks ||
        !DebugSetRunState || !DebugStep || !DebugGetState || !DebugMemRead32 || !DebugMemRead8 ||
        !DebugGetCPUDataPtr || !DebugBreakpointCommand || !DebugBreakpointTriggeredBy) {
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

    for (int waited_ms = 0; !g_dbg_ready; waited_ms += 10) {
        if (waited_ms > 20000) {
            fprintf(stderr, "debugger init callback never fired; aborting\n");
            _exit(1);
        }
        usleep(10 * 1000);
    }

    /* Debugger is armed and paused at the very first instruction. Install
     * breakpoints now, BEFORE letting anything run, so no hit can be
     * missed. m64p_breakpoint.endaddr documented as exclusive upper bound;
     * a single-word breakpoint uses address+4. */
    int bp_indices[NUM_EXEC_BPS];
    for (int i = 0; i < NUM_EXEC_BPS; i++) {
        m64p_breakpoint bp;
        memset(&bp, 0, sizeof(bp));
        bp.address = EXEC_BPS[i];
        bp.endaddr = EXEC_BPS[i] + 4;
        bp.flags = M64P_BKP_FLAG_ENABLED | M64P_BKP_FLAG_EXEC;
        int idx = DebugBreakpointCommand(M64P_BKP_CMD_ADD_STRUCT, 0, &bp);
        bp_indices[i] = idx;
        fprintf(stderr, "exec breakpoint %s @ 0x%08x -> index %d\n", EXEC_BP_NAMES[i], EXEC_BPS[i],
                idx);
        if (idx < 0) {
            fprintf(stderr, "failed to install exec breakpoint at 0x%08x\n", EXEC_BPS[i]);
            return 1;
        }
    }
    {
        m64p_breakpoint bp;
        memset(&bp, 0, sizeof(bp));
        bp.address = ADDR_FLAG;
        bp.endaddr = ADDR_FLAG + 4;
        bp.flags = M64P_BKP_FLAG_ENABLED | M64P_BKP_FLAG_WRITE;
        int idx = DebugBreakpointCommand(M64P_BKP_CMD_ADD_STRUCT, 0, &bp);
        fprintf(stderr, "write watchpoint flag-word @ 0x%08x -> index %d\n", ADDR_FLAG, idx);
        if (idx < 0) {
            fprintf(stderr, "failed to install write watchpoint at 0x%08x\n", ADDR_FLAG);
            return 1;
        }
    }

    fprintf(out,
            "{\"event\":\"header\",\"sequence\":0,"
            "\"normalized_rom_sha256\":\"%s\","
            "\"producer\":\"mupen-bfs v1 (mupen64plus-core 2.6.0 DEBUGGER=1 b0d68c2 "
            "pure-interpreter + rsp-hle, breakpoint-accelerated via public m64p_debugger API)\","
            "\"breakpoints\":[",
            digest_hex);
    for (int i = 0; i < NUM_EXEC_BPS; i++) {
        fprintf(out, "%s{\"kind\":\"exec\",\"name\":\"%s\",\"address\":%u}", i ? "," : "",
                EXEC_BP_NAMES[i], EXEC_BPS[i]);
    }
    fprintf(out, ",{\"kind\":\"write\",\"name\":\"flag-word\",\"address\":%u}]}\n", ADDR_FLAG);

    /* STEPPING, not RUNNING -- see the file header's MEASURED FINDING.
     * The core's own breakpoint check still identifies a hit via
     * DebugBreakpointTriggeredBy on every pause; we just no longer rely on
     * RUNNING to skip between them for free, because it does not. */
    rc = DebugSetRunState(M64P_DBG_RUNSTATE_STEPPING);
    if (rc != M64ERR_SUCCESS) {
        fprintf(stderr, "DebugSetRunState(STEPPING) -> %d; aborting\n", rc);
        _exit(1);
    }
    DebugStep();

    uint64_t seq = 1;
    unsigned long long hits = 0;
    unsigned long long resumes = 0;
    uint32_t last_flag = 0xffffffffu;
    int have_last_flag = 0;
    uint8_t last_mode = 0xffu;
    int have_last_mode = 0;

    for (;;) {
        uint32_t pc;
        wait_for_pause(&pc);
        resumes++;

        uint32_t trig_addr = 0, trig_flags = 0;
        DebugBreakpointTriggeredBy(&trig_flags, &trig_addr);
        uint32_t *pc_ptr = (uint32_t *)DebugGetCPUDataPtr(M64P_CPU_PC);
        uint32_t cur_pc = pc_ptr ? *pc_ptr : pc;

        uint32_t flag = DebugMemRead32(ADDR_FLAG);
        uint8_t mode = DebugMemRead8(ADDR_MODE);
        const char *bp_name = exec_bp_name(cur_pc);
        int is_write_hit = (trig_flags & M64P_BKP_FLAG_WRITE) != 0 && trig_addr == ADDR_FLAG;

        /* Ignore pauses that are neither a known exec breakpoint hit nor
         * the write watchpoint -- e.g. the very first init pause reported
         * before RUNNING was ever issued (pc == RECORD_START_PC is not one
         * of our exec bp addresses and the flag write watchpoint has not
         * fired yet), or step-mode remnants. Everything else is a real
         * hit and gets recorded + resumed. */
        if (!bp_name && !is_write_hit) {
            if (cur_pc == RECORD_START_PC) {
                fprintf(stderr, "initial entrypoint pause at 0x%08x (pre-run baseline)\n", cur_pc);
                fprintf(out,
                        "{\"event\":\"baseline\",\"sequence\":%llu,\"pc\":%u,\"flag\":%u,"
                        "\"mode\":%u}\n",
                        (unsigned long long)seq++, cur_pc, flag, mode);
                last_flag = flag;
                have_last_flag = 1;
                last_mode = mode;
                have_last_mode = 1;
                resume_running();
                continue;
            }
            /* NOT logged per-instruction: this branch executes once per
             * single-stepped instruction between hits (potentially
             * millions of times), and mupen_trace.c's own measured
             * baseline (216,000 steps/sec, no breakpoints) shows a
             * per-instruction fprintf(stderr, ...) here is a real,
             * measured throughput tax, not a style choice -- an earlier
             * version of this driver logged every non-hit pause and a
             * 15-second run only covered a few hundred instructions
             * because of it. A heartbeat every HEARTBEAT_RESUMES gives
             * liveness visibility instead. */
            if ((resumes % HEARTBEAT_RESUMES) == 0) {
                fprintf(stderr, "heartbeat: resumes=%llu cur_pc=0x%08x hits=%llu\n", resumes,
                        cur_pc, hits);
            }
            resume_running();
            if (resumes >= budget_resumes) {
                fprintf(stderr, "budget_resumes (%llu) exhausted without a named hit; aborting\n",
                        budget_resumes);
                break;
            }
            continue;
        }

        int flag_changed = !have_last_flag || flag != last_flag;
        int mode_changed = !have_last_mode || mode != last_mode;
        fprintf(out,
                "{\"event\":\"hit\",\"sequence\":%llu,\"pc\":%u,\"bp_name\":%s%s%s,"
                "\"trigger_flags\":%u,\"trigger_addr\":%u,\"flag\":%u,\"mode\":%u,"
                "\"flag_changed\":%s,\"mode_changed\":%s}\n",
                (unsigned long long)seq++, cur_pc, bp_name ? "\"" : "null",
                bp_name ? bp_name : "", bp_name ? "\"" : "", trig_flags, trig_addr, flag, mode,
                flag_changed ? "true" : "false", mode_changed ? "true" : "false");
        fflush(out);
        fprintf(stderr,
                "hit #%llu seq=%llu pc=0x%08x name=%s trig_flags=0x%x trig_addr=0x%08x flag=0x%x "
                "mode=0x%x%s%s\n",
                hits + 1, (unsigned long long)(seq - 1), cur_pc, bp_name ? bp_name : "(write)",
                trig_flags, trig_addr, flag, mode, flag_changed ? " FLAG_CHANGED" : "",
                mode_changed ? " MODE_CHANGED" : "");
        last_flag = flag;
        have_last_flag = 1;
        last_mode = mode;
        have_last_mode = 1;
        hits++;

        if (max_hits != 0 && hits >= max_hits) {
            fprintf(out, "{\"event\":\"end\",\"sequence\":%llu,\"completion\":\"completed\",\"hits\":%llu}\n",
                    (unsigned long long)seq, hits);
            fclose(out);
            fprintf(stderr, "reached max_hits=%llu; stopping\n", max_hits);
            pthread_mutex_lock(&g_lock);
            g_done = 1;
            pthread_cond_broadcast(&g_consumed);
            pthread_cond_broadcast(&g_cond);
            pthread_mutex_unlock(&g_lock);
            CoreDoCommand(M64CMD_STOP, 0, NULL);
            struct timespec grace = {0, 200 * 1000 * 1000};
            nanosleep(&grace, NULL);
            _exit(0);
        }
        if (resumes >= budget_resumes) {
            fprintf(out,
                    "{\"event\":\"end\",\"sequence\":%llu,\"completion\":\"budget_exhausted\","
                    "\"hits\":%llu}\n",
                    (unsigned long long)seq, hits);
            fclose(out);
            fprintf(stderr, "budget_resumes (%llu) exhausted after %llu hits; stopping\n",
                    budget_resumes, hits);
            pthread_mutex_lock(&g_lock);
            g_done = 1;
            pthread_cond_broadcast(&g_consumed);
            pthread_cond_broadcast(&g_cond);
            pthread_mutex_unlock(&g_lock);
            CoreDoCommand(M64CMD_STOP, 0, NULL);
            struct timespec grace = {0, 200 * 1000 * 1000};
            nanosleep(&grace, NULL);
            _exit(0);
        }
        resume_running();
    }

    fprintf(out, "{\"event\":\"end\",\"sequence\":%llu,\"completion\":\"aborted\",\"hits\":%llu}\n",
            (unsigned long long)seq, hits);
    fclose(out);
    pthread_mutex_lock(&g_lock);
    g_done = 1;
    pthread_cond_broadcast(&g_consumed);
    pthread_cond_broadcast(&g_cond);
    pthread_mutex_unlock(&g_lock);
    CoreDoCommand(M64CMD_STOP, 0, NULL);
    struct timespec grace = {0, 200 * 1000 * 1000};
    nanosleep(&grace, NULL);
    _exit(1);
}
