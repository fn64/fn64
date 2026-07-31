/* Deterministic Mupen64Plus input plugin for fn64 black-box traces.
 * Built and loaded out-of-tree; it is not part of the fn64 runtime. */
#include "m64p_common.h"
#include "m64p_plugin.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct { unsigned port; unsigned long long first, end; unsigned short buttons; signed char x, y; } phase_t;
static phase_t phases[256]; static unsigned count; static unsigned long long reads[4];

static void load_schedule(void) {
    const char *path = getenv("FN64_INPUT_SCHEDULE");
    if (!path) return;
    FILE *f = fopen(path, "rb"); if (!f) return;
    char line[256];
    while (fgets(line, sizeof line, f) && count < 256) {
        if (line[0] == '#' || strstr(line, "fn64.controller-input-schedule.v1")) continue;
        unsigned port; unsigned long long first, end; unsigned buttons; int x, y;
        if (sscanf(line, "%u %llu %llu %x %d %d", &port, &first, &end, &buttons, &x, &y) != 6) continue;
        if (port >= 4 || first >= end || x < -128 || x > 127 || y < -128 || y > 127) continue;
        phases[count++] = (phase_t){ port, first, end, (unsigned short)buttons, (signed char)x, (signed char)y };
    }
    fclose(f);
}

EXPORT m64p_error CALL PluginStartup(m64p_dynlib_handle core, void *ctx,
                                     void (*cb)(void *, int, const char *)) {
    (void)core; (void)ctx; (void)cb; load_schedule(); return M64ERR_SUCCESS;
}
EXPORT m64p_error CALL PluginShutdown(void) { return M64ERR_SUCCESS; }
EXPORT m64p_error CALL PluginGetVersion(m64p_plugin_type *type, int *version, int *api,
                                        const char **name, int *caps) {
    if (type) *type = M64PLUGIN_INPUT; if (version) *version = 0x00010000; if (api) *api = 0x00020101;
    if (name) *name = "fn64 deterministic input"; if (caps) *caps = 0; return M64ERR_SUCCESS;
}
EXPORT void CALL InitiateControllers(CONTROL_INFO info) { (void)info; memset(reads, 0, sizeof reads); }
EXPORT void CALL GetKeys(int control, BUTTONS *keys) {
    if (!keys || control < 0 || control >= 4) return;
    memset(keys, 0, sizeof *keys);
    unsigned long long ordinal = reads[control]++;
    for (unsigned i = 0; i < count; i++) {
        if (phases[i].port != (unsigned)control || ordinal < phases[i].first || ordinal >= phases[i].end) continue;
        unsigned short b = phases[i].buttons;
        keys->Value = b; keys->X_AXIS = phases[i].x; keys->Y_AXIS = phases[i].y; return;
    }
}
EXPORT void CALL ControllerCommand(int control, unsigned char *command) { (void)control; (void)command; }
EXPORT void CALL ReadController(int control, unsigned char *command) { (void)control; (void)command; }
EXPORT void CALL SDL_KeyDown(int mod, int key) { (void)mod; (void)key; }
EXPORT void CALL SDL_KeyUp(int mod, int key) { (void)mod; (void)key; }
EXPORT void CALL RenderCallback(void) {}
EXPORT void CALL SendVRUWord(uint16_t length, uint16_t *word, uint8_t lang) { (void)length; (void)word; (void)lang; }
EXPORT void CALL SetMicState(int state) { (void)state; }
EXPORT void CALL ReadVRUResults(uint16_t *error, uint16_t *num, uint16_t *mic, uint16_t *voice, uint16_t *length, uint16_t *matches) {
    if (error) *error = 0; if (num) *num = 0; if (mic) *mic = 0; if (voice) *voice = 0; if (length) *length = 0; if (matches) *matches = 0;
}
EXPORT void CALL ClearVRUWords(uint8_t length) { (void)length; }
EXPORT void CALL SetVRUWordMask(uint8_t length, uint8_t *mask) { (void)length; (void)mask; }
EXPORT int CALL RomOpen(void) { return 1; }
EXPORT void CALL RomClosed(void) {}
