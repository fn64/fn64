#include <stdint.h>

typedef void (*recomp_func_t)(uint8_t *rdram, void *ctx);
extern void fn64_c_recompiled_function_enter(recomp_func_t function);

void fn64_synthetic_recompiled_entry(uint8_t *rdram, void *ctx) {
    (void)rdram;
    (void)ctx;
    fn64_c_recompiled_function_enter(fn64_synthetic_recompiled_entry);
}

uint32_t fn64_synthetic_recompiled_step(uint32_t value) {
    return (value ^ UINT32_C(0x4e363452)) + UINT32_C(0x1020304);
}
