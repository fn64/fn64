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
 *     (the update callback reports the address about to execute). The public
 *     debugger advances a control transfer and its architectural delay slot
 *     atomically, so no separate pause exposes that delay-slot PC.
 *   - watched_table_write records: after every retired instruction the two
 *     watched cells (selector flag word 0x800a10b0, mode byte 0x80097fd8)
 *     are re-read via DebugMemRead32/8; every VALUE TRANSITION is emitted.
 *     With FN64_WATCH_VI=1, the fourteen standard VI MMIO words are polled
 *     through the same public API and their value transitions are emitted.
 *     FN64_WATCH_WORD=<aligned-address> adds one diagnostic RDRAM word poll;
 *     transitions are printed to stderr with the immediately preceding
 *     pause PC.  That PC is an observation boundary, not write attribution:
 *     asynchronous device work can become visible between adjacent pauses.
 *     These are observed-value records at instruction granularity -- they
 *     carry no write-PC attribution and a store that rewrites the same value
 *     is invisible.
 *   - end record: completion=completed with no exhaustiveness claim. The
 *     public debugger's atomic branch-plus-delay step means the pause-PC
 *     stream cannot claim every retired instruction. Execution before
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
 * Reproducibility: the trace contains sequence numbers only, never host
 * timestamps. Repeated captures must reproduce the exact observed bank-root
 * set before that set is admitted as scenario coverage. Asynchronous interrupt
 * delivery can move an exception entry by adjacent guest instructions, so
 * byte identity is not an execution-root authority. FN64_WATCH_VI is
 * diagnostic only: public-debugger MMIO polling can likewise place a
 * transition on either side of one adjacent pause.
 *
 * Build (macOS, Homebrew mupen64plus headers; CommonCrypto for SHA-256):
 *   cc -O2 -Wall -Wextra -o mupen_trace mupen_trace.c \
 *      -I/opt/homebrew/Cellar/mupen64plus/2.6.0/include -lpthread
 * Run:
 *   ./mupen_trace <core.dylib> <rom.z64> <rsp.dylib> <out.jsonl> <steps> <trace_id> <boot-context.json>
 *
 * The final output is a separate `fn64.boot-context.v1` observation captured
 * while the debugger is paused immediately before the normalized ROM header
 * entry executes. It retains all GPRs, HI/LO, the public debugger's complete
 * 32-slot CP0 image, exact ROM/IPL3 identities, and header-derived TV
 * standard. The block lane consumes this file instead of inventing zeroed
 * IPL3 state.
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
#define VI_MMIO_BASE 0xa4400000u
#define VI_REGISTER_COUNT 14u
#define MI_INTR_REG 0xa4300008u
#define MI_MASK_REG 0xa430000cu

/* Resident-bank attribution window; see the header comment for the
 * boot-copy justification. */
#define RESIDENT_VA_START 0x80000400u
#define RESIDENT_VA_END 0x80056670u

/* Give up if the entrypoint pause never arrives within this many pre-window
 * steps (loud failure, not a hang). */
#define MAX_SKIP_STEPS 40000000ull

static ptr_DebugMemRead32 DebugMemRead32;
static ptr_DebugMemRead8 DebugMemRead8;
static ptr_DebugGetCPUDataPtr DebugGetCPUDataPtr;
static ptr_DebugGetState DebugGetState;
static ptr_DebugSetRunState DebugSetRunState;
static ptr_DebugStep DebugStep;
static ptr_CoreDoCommand g_do_command;

static int hash_optional_file(const char *path, char out[CC_SHA256_DIGEST_LENGTH * 2 + 1]) {
    out[0] = '\0';
    if (!path) return 0;
    FILE *file = fopen(path, "rb");
    if (!file) return -1;
    CC_SHA256_CTX context; CC_SHA256_Init(&context);
    unsigned char buffer[8192]; size_t length;
    while ((length = fread(buffer, 1, sizeof buffer, file)) != 0) CC_SHA256_Update(&context, buffer, (CC_LONG)length);
    if (ferror(file)) { fclose(file); return -1; }
    unsigned char digest[CC_SHA256_DIGEST_LENGTH]; CC_SHA256_Final(digest, &context); fclose(file);
    for (int i = 0; i < CC_SHA256_DIGEST_LENGTH; i++) snprintf(out + i * 2, 3, "%02x", digest[i]);
    return 1;
}

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
 * report can never be overwritten -- the pause-PC stream must not acquire
 * host-scheduling gaps on top of the debugger's step boundary. */
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

/* Wait for the next pause report with exactly one outstanding DebugStep.
 *
 * A timeout re-kick can race the emulator between retiring the prior
 * instruction and publishing its update callback: the core retains that
 * second step signal, consumes it at the next pause, and retires another
 * instruction before the main thread observes the first pause. That exact
 * interleaving lost pre-window reports and made captured CP0 Count depend on
 * host scheduling. A stalled callback now fails loudly instead of creating a
 * second step token. */
static int wait_for_pause(uint32_t *pc_out) {
    pthread_mutex_lock(&g_lock);
    while (!g_have_pc) {
        struct timespec deadline;
        clock_gettime(CLOCK_REALTIME, &deadline);
        deadline.tv_sec += 20;
        int wait_result = pthread_cond_timedwait(&g_cond, &g_lock, &deadline);
        if (wait_result != 0 && !g_have_pc) {
            pthread_mutex_unlock(&g_lock);
            int debugger_state = DebugGetState ? DebugGetState(M64P_DBG_RUN_STATE) : -1;
            fprintf(stderr,
                    "debugger update callback stalled with one outstanding step "
                    "(wait=%d state=%d)\n",
                    wait_result, debugger_state);
            _exit(1);
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

/* Optional overlay attribution windows, supplied out-of-band by
 * FN64_OVERLAY_WINDOWS as lines of "<bank> <va_start> <va_end>" (hex or
 * decimal). Without the file, behaviour is exactly as before: only the
 * resident window is named and every other PC stays bank-unknown.
 *
 * WHY A FILE AND NOT A BUILT-IN TABLE: the windows come from fn64-discover's
 * proven RomMapping facts for the ROM under trace. Hardcoding them here would
 * put a per-game claim in the producer, which is the thing this boundary
 * exists to avoid -- and the producer cannot verify them anyway.
 *
 * ALIASING IS FAIL-CLOSED: overlay images share load slots (WM2000 puts two
 * images at 0x800e1b90 and two more at 0x8011c900). A PC inside a slot claimed
 * by more than one window cannot be attributed by VA alone -- knowing WHICH
 * image is resident needs the active load generation, which this producer does
 * not observe. Such windows are rejected at load time and their PCs stay
 * unknown, exactly as today. Only a slot claimed by exactly one image is
 * named. */
#define MAX_OVERLAY_WINDOWS 32
static struct {
    char bank[64];
    uint32_t va_start;
    uint32_t va_end;
} g_overlay_windows[MAX_OVERLAY_WINDOWS];
static unsigned g_overlay_window_count;

static void load_overlay_windows(void) {
    const char *path = getenv("FN64_OVERLAY_WINDOWS");
    if (!path) return;
    FILE *file = fopen(path, "rb");
    if (!file) {
        fprintf(stderr, "cannot open FN64_OVERLAY_WINDOWS %s\n", path);
        exit(2);
    }
    char line[256];
    unsigned parsed = 0;
    while (fgets(line, sizeof line, file) && parsed < MAX_OVERLAY_WINDOWS) {
        char bank[64];
        unsigned long long start = 0, end = 0;
        if (line[0] == '#') continue;
        if (sscanf(line, "%63s %llx %llx", bank, &start, &end) != 3) continue;
        if (start >= end || start > 0xffffffffull || end > 0xffffffffull) continue;
        snprintf(g_overlay_windows[parsed].bank, sizeof g_overlay_windows[parsed].bank,
                 "%s", bank);
        g_overlay_windows[parsed].va_start = (uint32_t)start;
        g_overlay_windows[parsed].va_end = (uint32_t)end;
        parsed++;
    }
    fclose(file);
    /* Drop every window whose slot another window also claims. */
    unsigned kept = 0;
    for (unsigned i = 0; i < parsed; i++) {
        int aliased = 0;
        for (unsigned j = 0; j < parsed; j++) {
            if (i == j) continue;
            if (g_overlay_windows[i].va_start < g_overlay_windows[j].va_end &&
                g_overlay_windows[j].va_start < g_overlay_windows[i].va_end) {
                aliased = 1;
                break;
            }
        }
        if (aliased) {
            fprintf(stderr, "overlay window %s rejected: slot shared with another image\n",
                    g_overlay_windows[i].bank);
            continue;
        }
        g_overlay_windows[kept++] = g_overlay_windows[i];
    }
    g_overlay_window_count = kept;
    fprintf(stderr, "overlay attribution windows admitted: %u of %u\n", kept, parsed);
}

static const char *overlay_bank_for_pc(uint32_t pc) {
    for (unsigned i = 0; i < g_overlay_window_count; i++) {
        if (pc >= g_overlay_windows[i].va_start && pc < g_overlay_windows[i].va_end) {
            return g_overlay_windows[i].bank;
        }
    }
    return NULL;
}

static void emit_executed_pc(FILE *out, uint64_t seq, uint32_t pc) {
    const char *overlay = overlay_bank_for_pc(pc);
    if (overlay) {
        fprintf(out,
                "{\"event\":\"executed_pc\",\"sequence\":%llu,\"pc\":{\"address\":%u,"
                "\"bank\":{\"status\":\"known\",\"bank\":\"%s\",\"activation\":0}}}\n",
                (unsigned long long)seq, pc, overlay);
    } else if (pc_in_resident(pc)) {
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

static void emit_vi_watch(FILE *out, uint64_t seq, unsigned int index, uint32_t value) {
    char watch_id[48];
    uint32_t address = VI_MMIO_BASE + index * 4;
    snprintf(watch_id, sizeof(watch_id), "vi-register-%02u-0x%08x", index, address);
    emit_watch(out, seq, watch_id, address, "u32", value);
}

static const char *tv_standard_for_destination(uint8_t code) {
    switch (code) {
    case 'B':
        return "mpal";
    case 'C': case 'E': case 'G': case 'J': case 'K': case 'N':
        return "ntsc";
    case 'D': case 'F': case 'H': case 'I': case 'L': case 'P':
    case 'S': case 'U': case 'W': case 'X': case 'Y': case 'Z':
        return "pal";
    case 0: case 'A': {
        const char *explicit_standard = getenv("FN64_TV_STANDARD");
        if (explicit_standard &&
            (!strcmp(explicit_standard, "ntsc") ||
             !strcmp(explicit_standard, "pal") ||
             !strcmp(explicit_standard, "mpal")))
            return explicit_standard;
        fprintf(stderr,
                "region-free destination code requires FN64_TV_STANDARD=ntsc|pal|mpal\n");
        return NULL;
    }
    default:
        fprintf(stderr, "unknown ROM destination code 0x%02x\n", code);
        return NULL;
    }
}

static void emit_boot_context(const char *path, const char *rom_sha256,
                              const char *ipl3_sha256, uint8_t destination_code,
                              const char *tv_standard, uint32_t entry_pc) {
    uint64_t *gprs = (uint64_t *)DebugGetCPUDataPtr(M64P_CPU_REG_REG);
    uint64_t *hi = (uint64_t *)DebugGetCPUDataPtr(M64P_CPU_REG_HI);
    uint64_t *lo = (uint64_t *)DebugGetCPUDataPtr(M64P_CPU_REG_LO);
    uint32_t *cop0 = (uint32_t *)DebugGetCPUDataPtr(M64P_CPU_REG_COP0);
    if (!gprs || !hi || !lo || !cop0) {
        fprintf(stderr, "public debugger CPU register pointer returned NULL\n");
        _exit(1);
    }
    FILE *out = fopen(path, "wbx");
    if (!out) {
        fprintf(stderr, "cannot create boot-context output %s (output must not exist)\n", path);
        _exit(1);
    }
    fprintf(out,
            "{\"schema\":\"fn64.boot-context.v1\","
            "\"producer\":\"mupen-trace v1 public m64p_debugger header-entry capture\","
            "\"normalized_rom_sha256\":\"%s\","
            "\"cic\":{\"ipl3_sha256\":\"%s\"},"
            "\"region\":{\"destination_code\":%u,\"tv_standard\":\"%s\"},"
            "\"entry_pc\":%u,\"gprs\":[",
            rom_sha256, ipl3_sha256, destination_code, tv_standard, entry_pc);
    for (int index = 0; index < 32; index++)
        fprintf(out, "%s%llu", index ? "," : "", (unsigned long long)gprs[index]);
    fprintf(out, "],\"hi\":%llu,\"lo\":%llu,\"cp0\":{\"registers\":[",
            (unsigned long long)*hi, (unsigned long long)*lo);
    for (int index = 0; index < 32; index++)
        fprintf(out, "%s%u", index ? "," : "", cop0[index]);
    fprintf(out, "]}}\n");
    if (fclose(out) != 0) {
        fprintf(stderr, "closing boot-context output %s failed\n", path);
        _exit(1);
    }
}

static void emit_cpu_snapshot(const char *path, const char *rom_sha256,
                              uint32_t pc, unsigned long long retired) {
    uint64_t *gprs = (uint64_t *)DebugGetCPUDataPtr(M64P_CPU_REG_REG);
    uint64_t *hi = (uint64_t *)DebugGetCPUDataPtr(M64P_CPU_REG_HI);
    uint64_t *lo = (uint64_t *)DebugGetCPUDataPtr(M64P_CPU_REG_LO);
    uint32_t *cop0 = (uint32_t *)DebugGetCPUDataPtr(M64P_CPU_REG_COP0);
    if (!gprs || !hi || !lo || !cop0) {
        fprintf(stderr, "public debugger CPU register pointer returned NULL at target snapshot\n");
        _exit(1);
    }
    FILE *out = fopen(path, "wbx");
    if (!out) {
        fprintf(stderr, "cannot create CPU-snapshot output %s (output must not exist)\n", path);
        _exit(1);
    }
    fprintf(out,
            "{\"schema\":\"fn64.cpu-snapshot.v1\","
            "\"producer\":\"mupen-trace v1 public m64p_debugger target-PC capture\","
            "\"normalized_rom_sha256\":\"%s\",\"pc\":%u,"
            "\"retired_instructions\":%llu,\"gprs\":[",
            rom_sha256, pc, retired);
    for (int index = 0; index < 32; index++)
        fprintf(out, "%s%llu", index ? "," : "", (unsigned long long)gprs[index]);
    fprintf(out, "],\"hi\":%llu,\"lo\":%llu,\"cp0\":{\"registers\":[",
            (unsigned long long)*hi, (unsigned long long)*lo);
    for (int index = 0; index < 32; index++)
        fprintf(out, "%s%u", index ? "," : "", cop0[index]);
    fprintf(out, "]}}\n");
    if (fclose(out) != 0) {
        fprintf(stderr, "closing CPU-snapshot output %s failed\n", path);
        _exit(1);
    }
}

static void emit_executable_image(const char *path, const char *rom_sha256,
                                  const char *image_id, uint32_t capture_pc,
                                  uint32_t first_executed_pc,
                                  uint32_t va_start, uint32_t word_count,
                                  unsigned long long retired) {
    size_t byte_len = (size_t)word_count * 4;
    unsigned char *bytes = malloc(byte_len);
    uint32_t *words = malloc((size_t)word_count * sizeof(*words));
    if (!bytes || !words) {
        fprintf(stderr, "allocating executable-image snapshot failed\n");
        _exit(1);
    }
    for (uint32_t index = 0; index < word_count; index++) {
        uint32_t word = DebugMemRead32(va_start + index * 4);
        words[index] = word;
        bytes[index * 4 + 0] = (unsigned char)(word >> 24);
        bytes[index * 4 + 1] = (unsigned char)(word >> 16);
        bytes[index * 4 + 2] = (unsigned char)(word >> 8);
        bytes[index * 4 + 3] = (unsigned char)word;
    }
    unsigned char digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256(bytes, (CC_LONG)byte_len, digest);
    char digest_hex[CC_SHA256_DIGEST_LENGTH * 2 + 1];
    for (int index = 0; index < CC_SHA256_DIGEST_LENGTH; index++)
        snprintf(digest_hex + index * 2, 3, "%02x", digest[index]);

    FILE *out = fopen(path, "wbx");
    if (!out) {
        fprintf(stderr, "cannot create executable-image output %s (output must not exist)\n", path);
        _exit(1);
    }
    fprintf(out,
            "{\"schema\":\"fn64.executable-image.v1\","
            "\"producer\":\"mupen-trace v1 public m64p_debugger target-PC capture\","
            "\"normalized_rom_sha256\":\"%s\",\"image_id\":\"%s\","
            "\"lineage\":\"cpu_produced\",\"generation\":0,"
            "\"capture_pc\":%u,\"first_executed_pc\":%u,"
            "\"retired_instructions\":%llu,\"va_start\":%u,\"byte_len\":%zu,"
            "\"sha256\":\"%s\",\"words\":[",
            rom_sha256, image_id, capture_pc, first_executed_pc, retired, va_start, byte_len,
            digest_hex);
    for (uint32_t index = 0; index < word_count; index++)
        fprintf(out, "%s%u", index ? "," : "", words[index]);
    fprintf(out, "]}\n");
    if (fclose(out) != 0) {
        fprintf(stderr, "closing executable-image output %s failed\n", path);
        _exit(1);
    }
    free(words);
    free(bytes);
}

static void complete_trace(FILE *out, const char *out_path, uint64_t seq,
                           unsigned long long recorded) {
    fprintf(out,
            "{\"event\":\"end\",\"sequence\":%llu,\"completion\":\"completed\","
            "\"exhaustiveness\":[]}\n",
            (unsigned long long)seq);
    if (fclose(out) != 0) {
        fprintf(stderr, "closing %s failed\n", out_path);
        _exit(1);
    }
    fprintf(stderr, "trace complete: %llu executed-pc records, final sequence %llu\n",
            recorded, (unsigned long long)seq);
    /* The core is paused inside its own stepping wait on the EXECUTE thread.
     * The documented shutdown (RUNNING + STOP) is attempted; if the exec
     * thread does not return promptly, the process exits after the already-
     * closed trace and capture artifacts are durable. */
    pthread_mutex_lock(&g_lock);
    g_done = 1;
    pthread_cond_broadcast(&g_consumed);
    pthread_cond_broadcast(&g_cond);
    pthread_mutex_unlock(&g_lock);
    DebugSetRunState(M64P_DBG_RUNSTATE_RUNNING);
    DebugStep();
    g_do_command(M64CMD_STOP, 0, NULL);
    struct timespec grace = {0, 200 * 1000 * 1000};
    nanosleep(&grace, NULL);
    _exit(0);
}

int main(int argc, char **argv) {
    if (argc != 8) {
        fprintf(stderr,
                "usage: %s <core.dylib> <rom.z64> <rsp.dylib> <out.jsonl> <steps> <trace_id> <boot-context.json>\n",
                argv[0]);
        return 2;
    }
    load_overlay_windows();
    const char *core_path = argv[1];
    const char *rom_path = argv[2];
    const char *rsp_path = argv[3];
    const char *out_path = argv[4];
    unsigned long long steps = strtoull(argv[5], NULL, 10);
    const char *trace_id = argv[6];
    const char *boot_context_path = argv[7];
    const char *snapshot_pc_text = getenv("FN64_CPU_SNAPSHOT_PC");
    const char *snapshot_path = getenv("FN64_CPU_SNAPSHOT");
    uint32_t snapshot_pc = 0;
    int snapshot_pending = 0;
    if ((snapshot_pc_text == NULL) != (snapshot_path == NULL)) {
        fprintf(stderr, "FN64_CPU_SNAPSHOT_PC and FN64_CPU_SNAPSHOT must be set together\n");
        return 2;
    }
    if (snapshot_pc_text) {
        char *end = NULL;
        unsigned long parsed = strtoul(snapshot_pc_text, &end, 0);
        if (!snapshot_pc_text[0] || !end || *end || parsed > UINT32_MAX || (parsed & 3)) {
            fprintf(stderr, "invalid aligned FN64_CPU_SNAPSHOT_PC value %s\n", snapshot_pc_text);
            return 2;
        }
        snapshot_pc = (uint32_t)parsed;
        snapshot_pending = 1;
    }
    const char *image_pc_text = getenv("FN64_EXECUTABLE_IMAGE_PC");
    const char *image_start_text = getenv("FN64_EXECUTABLE_IMAGE_START");
    const char *image_words_text = getenv("FN64_EXECUTABLE_IMAGE_WORDS");
    const char *image_path = getenv("FN64_EXECUTABLE_IMAGE");
    const char *image_id = getenv("FN64_EXECUTABLE_IMAGE_ID");
    const char *image_first_pc_text = getenv("FN64_EXECUTABLE_IMAGE_FIRST_PC");
    int capture_only = getenv("FN64_CAPTURE_ONLY") != NULL;
    int stop_after_image = getenv("FN64_STOP_AFTER_IMAGE") != NULL;
    const char *watch_word_text = getenv("FN64_WATCH_WORD");
    uint32_t image_pc = 0;
    uint32_t image_first_pc = 0;
    uint32_t image_start = 0;
    uint32_t image_words = 0;
    int image_pending = 0;
    uint32_t watch_word_address = 0;
    int watch_word = 0;
    if (watch_word_text) {
        char *end = NULL;
        unsigned long parsed = strtoul(watch_word_text, &end, 0);
        if (!watch_word_text[0] || !end || *end || parsed > UINT32_MAX || (parsed & 3)) {
            fprintf(stderr, "invalid aligned FN64_WATCH_WORD value %s\n", watch_word_text);
            return 2;
        }
        watch_word_address = (uint32_t)parsed;
        watch_word = 1;
    }
    int image_options = (image_pc_text != NULL) + (image_start_text != NULL) +
                        (image_words_text != NULL) + (image_path != NULL) +
                        (image_id != NULL);
    if (image_options != 0 && image_options != 5) {
        fprintf(stderr, "the five required FN64_EXECUTABLE_IMAGE_* options must be set together\n");
        return 2;
    }
    if (image_options == 5) {
        char *pc_end = NULL;
        char *start_end = NULL;
        char *words_end = NULL;
        unsigned long parsed_pc = strtoul(image_pc_text, &pc_end, 0);
        unsigned long parsed_start = strtoul(image_start_text, &start_end, 0);
        unsigned long parsed_words = strtoul(image_words_text, &words_end, 0);
        char *first_pc_end = NULL;
        unsigned long parsed_first_pc = image_first_pc_text
            ? strtoul(image_first_pc_text, &first_pc_end, 0)
            : parsed_pc;
        if (!image_pc_text[0] || !pc_end || *pc_end || parsed_pc > UINT32_MAX ||
            (parsed_pc & 3) || !image_start_text[0] || !start_end || *start_end ||
            parsed_start > UINT32_MAX || (parsed_start & 3) || !image_words_text[0] ||
            !words_end || *words_end || parsed_words == 0 || parsed_words > 262144 ||
            parsed_start > UINT32_MAX - (parsed_words * 4) || !image_id[0] ||
            (image_first_pc_text && (!image_first_pc_text[0] || !first_pc_end || *first_pc_end)) ||
            parsed_first_pc > UINT32_MAX || (parsed_first_pc & 3) ||
            parsed_first_pc < parsed_start || parsed_first_pc >= parsed_start + parsed_words * 4) {
            fprintf(stderr, "invalid FN64_EXECUTABLE_IMAGE_* options\n");
            return 2;
        }
        image_pc = (uint32_t)parsed_pc;
        image_first_pc = (uint32_t)parsed_first_pc;
        image_start = (uint32_t)parsed_start;
        image_words = (uint32_t)parsed_words;
        image_pending = 1;
    }
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
    if (romlen < 0x1000) {
        fprintf(stderr, "ROM is too short to contain the complete IPL3 image\n");
        return 1;
    }
    unsigned char ipl3_digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256(rombuf + 0x40, 0x1000 - 0x40, ipl3_digest);
    char ipl3_digest_hex[CC_SHA256_DIGEST_LENGTH * 2 + 1];
    for (int i = 0; i < CC_SHA256_DIGEST_LENGTH; i++)
        snprintf(ipl3_digest_hex + i * 2, 3, "%02x", ipl3_digest[i]);
    uint32_t record_start_pc = ((uint32_t)rombuf[8] << 24) |
                               ((uint32_t)rombuf[9] << 16) |
                               ((uint32_t)rombuf[10] << 8) |
                               (uint32_t)rombuf[11];
    uint8_t destination_code = rombuf[0x3e];
    const char *tv_standard = tv_standard_for_destination(destination_code);
    if (!tv_standard)
        return 1;

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
    DebugGetCPUDataPtr = (ptr_DebugGetCPUDataPtr)dlsym(core, "DebugGetCPUDataPtr");
    DebugGetState = (ptr_DebugGetState)dlsym(core, "DebugGetState");
    if (!CoreStartup || !CoreAttachPlugin || !CoreDoCommand || !DebugSetCallbacks ||
        !DebugSetRunState || !DebugStep || !DebugMemRead32 || !DebugMemRead8 ||
        !DebugGetCPUDataPtr) {
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
    /* Some DEBUGGER=1 core builds construct the debugger only during ROM_OPEN;
     * re-register the public callbacks after that lifecycle boundary as well.
     * The call is idempotent for cores that initialized it earlier. */
    rc = DebugSetCallbacks(dbg_init, dbg_update, dbg_vi);
    if (rc != M64ERR_SUCCESS) {
        fprintf(stderr, "DebugSetCallbacks(after ROM_OPEN) -> %d\n", rc);
        return 1;
    }
    void *rsp_h = dlopen(rsp_path, RTLD_NOW | RTLD_LOCAL);
    if (!rsp_h) {
        fprintf(stderr, "dlopen rsp failed: %s\n", dlerror());
        return 1;
    }
    ptr_PluginStartup PluginStartup = (ptr_PluginStartup)dlsym(rsp_h, "PluginStartup");
    if (!PluginStartup || PluginStartup(core, NULL, debug_cb) != M64ERR_SUCCESS) {
        fprintf(stderr, "rsp plugin attach failed\n");
        return 1;
    }
    /* Core-internal dummy gfx/audio plugins. Input is optionally replaced by
     * the deterministic fn64 plugin selected through FN64_INPUT_PLUGIN. */
    void *input_h = NULL;
    const char *input_path = getenv("FN64_INPUT_PLUGIN");
    if (input_path) {
        input_h = dlopen(input_path, RTLD_NOW | RTLD_LOCAL);
        if (!input_h) { fprintf(stderr, "input plugin dlopen failed: %s\n", dlerror()); return 1; }
        ptr_PluginStartup InputStartup = (ptr_PluginStartup)dlsym(input_h, "PluginStartup");
        if (!InputStartup || InputStartup(core, NULL, debug_cb) != M64ERR_SUCCESS) {
            fprintf(stderr, "input plugin attach failed\n"); return 1;
        }
    }
    if (CoreAttachPlugin(M64PLUGIN_GFX, NULL) != M64ERR_SUCCESS ||
        CoreAttachPlugin(M64PLUGIN_AUDIO, NULL) != M64ERR_SUCCESS ||
        (input_h == NULL && CoreAttachPlugin(M64PLUGIN_INPUT, NULL) != M64ERR_SUCCESS) ||
        (input_h != NULL && CoreAttachPlugin(M64PLUGIN_INPUT, input_h) != M64ERR_SUCCESS)) {
        fprintf(stderr, "dummy plugin attach failed\n");
        return 1;
    }
    /* The frontend validates attachment order: RSP follows gfx/audio/input. */
    if (CoreAttachPlugin(M64PLUGIN_RSP, rsp_h) != M64ERR_SUCCESS) {
        fprintf(stderr, "rsp plugin attach failed\n");
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
    const char *prelude_env = getenv("FN64_CONTINUOUS_PRELUDE_MS");
    unsigned long prelude_ms = prelude_env ? strtoul(prelude_env, NULL, 10) : 0;
    uint32_t pending_pause_pc = 0;
    int have_pending_pause = 0;
    if (prelude_ms > 0) {
        if (snapshot_pending || image_pending) {
            fprintf(stderr,
                    "FN64_CONTINUOUS_PRELUDE_MS cannot preserve target-PC capture boundaries\n");
            _exit(2);
        }
        if (prelude_ms > 120000) {
            fprintf(stderr, "FN64_CONTINUOUS_PRELUDE_MS exceeds 120000 ms\n");
            _exit(1);
        }
        rc = DebugSetRunState(M64P_DBG_RUNSTATE_RUNNING);
        if (rc != M64ERR_SUCCESS) {
            fprintf(stderr, "DebugSetRunState(RUNNING) -> %d; aborting\n", rc);
            _exit(1);
        }
        usleep((useconds_t)prelude_ms * 1000U);
        rc = CoreDoCommand(M64CMD_PAUSE, 0, NULL);
        if (rc != M64ERR_SUCCESS) {
            fprintf(stderr, "CoreDoCommand(PAUSE) -> %d; aborting\n", rc);
            _exit(1);
        }
        wait_for_pause(&pending_pause_pc);
        have_pending_pause = 1;
        fprintf(stderr, "continuous prelude paused after %lu ms\n", prelude_ms);
    }
    rc = DebugSetRunState(M64P_DBG_RUNSTATE_STEPPING);
    if (rc != M64ERR_SUCCESS) {
        fprintf(stderr, "DebugSetRunState(STEPPING) -> %d; aborting\n", rc);
        _exit(1);
    }
    if (!have_pending_pause)
        DebugStep();

    const char *schedule_path = getenv("FN64_INPUT_SCHEDULE");
    char schedule_sha256[CC_SHA256_DIGEST_LENGTH * 2 + 1];
    int schedule_status = hash_optional_file(schedule_path, schedule_sha256);
    if (schedule_status < 0) { fprintf(stderr, "cannot hash FN64_INPUT_SCHEDULE\n"); return 1; }
    fprintf(out,
            "{\"event\":\"header\",\"sequence\":0,\"schema_version\":1,"
            "\"normalized_rom_sha256\":\"%s\",\"trace_id\":\"%s\","
            "\"controller_input_schedule_sha256\":%s%s%s,"
            "\"producer\":\"mupen-trace v1 (mupen64plus-core 2.6.0 DEBUGGER=1 b0d68c2 "
            "pure-interpreter + rsp-hle, single-step via public m64p_debugger API)\"}\n",
            digest_hex, trace_id,
            schedule_status > 0 ? "\"" : "null",
            schedule_status > 0 ? schedule_sha256 : "",
            schedule_status > 0 ? "\"" : "");

    uint64_t seq = 1;
    uint64_t skip_steps = 0;
    uint32_t capture_start_pc = record_start_pc;
    const char *fast_forward_pc = getenv("FN64_FAST_FORWARD_PC");
    if (fast_forward_pc && *fast_forward_pc) {
        char *end = NULL;
        unsigned long parsed = strtoul(fast_forward_pc, &end, 0);
        if (*end != '\0' || parsed > UINT32_MAX || (parsed & 3U) != 0 ||
            parsed < RESIDENT_VA_START || parsed >= RESIDENT_VA_END) {
            fprintf(stderr, "FN64_FAST_FORWARD_PC must be an aligned resident VA\n");
            _exit(1);
        }
        capture_start_pc = (uint32_t)parsed;
        fprintf(stderr, "fast-forward capture start PC 0x%08x\n", capture_start_pc);
    }
    unsigned long long recorded = 0;
    int recording = 0;
    int have_prev = 0;
    uint32_t prev_pc = 0;
    uint32_t last_flag = 0;
    uint32_t last_mode = 0;
    uint32_t last_vi[VI_REGISTER_COUNT] = {0};
    int watch_vi = getenv("FN64_WATCH_VI") != NULL;
    uint32_t last_mi_intr = 0;
    uint32_t last_mi_mask = 0;
    int watch_rcp_interrupts = getenv("FN64_WATCH_RCP_INTERRUPTS") != NULL;
    uint32_t last_watch_word = 0;

    for (;;) {
        uint32_t pc;
        if (have_pending_pause) {
            pc = pending_pause_pc;
            have_pending_pause = 0;
        } else {
            wait_for_pause(&pc);
        }

        int entered_recording = 0;
        if (!recording) {
            if (pc == capture_start_pc) {
                recording = 1;
                entered_recording = 1;
                emit_boot_context(boot_context_path, digest_hex, ipl3_digest_hex,
                                  destination_code, tv_standard, capture_start_pc);
                /* Baseline observed values at the entrypoint pause. */
                last_flag = DebugMemRead32(ADDR_FLAG);
                last_mode = DebugMemRead8(ADDR_MODE);
                if (watch_word) {
                    last_watch_word = DebugMemRead32(watch_word_address);
                    fprintf(stderr,
                            "watch word 0x%08x baseline=0x%08x at entrypoint pause\n",
                            watch_word_address, last_watch_word);
                }
                emit_watch(out, seq++, "selector-flag-0x800a10b0", ADDR_FLAG, "u32", last_flag);
                emit_watch(out, seq++, "mode-byte-0x80097fd8", ADDR_MODE, "u8", last_mode);
                if (watch_vi) {
                    for (unsigned int index = 0; index < VI_REGISTER_COUNT; index++) {
                        uint32_t address = VI_MMIO_BASE + index * 4;
                        last_vi[index] = DebugMemRead32(address);
                        emit_vi_watch(out, seq++, index, last_vi[index]);
                    }
                }
                if (watch_rcp_interrupts) {
                    last_mi_intr = DebugMemRead32(MI_INTR_REG);
                    last_mi_mask = DebugMemRead32(MI_MASK_REG);
                    emit_watch(out, seq++, "mi-intr-0xa4300008", MI_INTR_REG, "u32",
                               last_mi_intr);
                    emit_watch(out, seq++, "mi-mask-0xa430000c", MI_MASK_REG, "u32",
                               last_mi_mask);
                }
                prev_pc = pc;
                have_prev = 1;
                fprintf(stderr, "entrypoint pause at 0x%08x after %llu pre-window steps\n", pc,
                        (unsigned long long)skip_steps);
            } else {
                if (++skip_steps > MAX_SKIP_STEPS) {
                    fprintf(stderr,
                            "entrypoint 0x%08x never reached within %llu steps; aborting\n",
                            capture_start_pc, (unsigned long long)MAX_SKIP_STEPS);
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
        }

        /* Target captures observe a pause before its instruction executes.
         * The recording-start pause is itself eligible: fast-forwarding to
         * the same PC as a requested image/CPU snapshot must not consume that
         * one-shot boundary before the capture checks run. */
        if (snapshot_pending && pc == snapshot_pc) {
            emit_cpu_snapshot(snapshot_path, digest_hex, pc, recorded);
            snapshot_pending = 0;
            fprintf(stderr,
                    "CPU snapshot captured before 0x%08x after %llu retired window instructions\n",
                    pc, recorded);
        }
        if (image_pending && pc == image_pc) {
            emit_executable_image(image_path, digest_hex, image_id, pc, image_first_pc, image_start,
                                  image_words, recorded);
            image_pending = 0;
            fprintf(stderr,
                    "executable image %s captured before 0x%08x after %llu retired window instructions\n",
                    image_id, pc, recorded);
            if (stop_after_image) {
                if (entered_recording)
                    complete_trace(out, out_path, seq, recorded);
                steps = recorded;
            }
        }
        if (entered_recording) {
            DebugStep();
            continue;
        }

        /* This pause proves the instruction at prev_pc retired. */
        if (have_prev) {
            if (!capture_only)
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
            if (watch_word) {
                uint32_t value = DebugMemRead32(watch_word_address);
                if (value != last_watch_word) {
                    fprintf(stderr,
                            "watch word 0x%08x transition 0x%08x -> 0x%08x "
                            "observed after pc=0x%08x retired=%llu\n",
                            watch_word_address, last_watch_word, value, prev_pc,
                            recorded);
                    last_watch_word = value;
                }
            }
            if (watch_vi) {
                for (unsigned int index = 0; index < VI_REGISTER_COUNT; index++) {
                    uint32_t address = VI_MMIO_BASE + index * 4;
                    uint32_t value = DebugMemRead32(address);
                    if (value != last_vi[index]) {
                        emit_vi_watch(out, seq++, index, value);
                        last_vi[index] = value;
                    }
                }
            }
            if (watch_rcp_interrupts) {
                uint32_t mi_intr = DebugMemRead32(MI_INTR_REG);
                uint32_t mi_mask = DebugMemRead32(MI_MASK_REG);
                if (mi_intr != last_mi_intr) {
                    emit_watch(out, seq++, "mi-intr-0xa4300008", MI_INTR_REG, "u32", mi_intr);
                    last_mi_intr = mi_intr;
                }
                if (mi_mask != last_mi_mask) {
                    emit_watch(out, seq++, "mi-mask-0xa430000c", MI_MASK_REG, "u32", mi_mask);
                    last_mi_mask = mi_mask;
                }
            }
        }
        prev_pc = pc;

        if (recorded >= steps) {
            complete_trace(out, out_path, seq, recorded);
        }
        DebugStep();
    }
}
