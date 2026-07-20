#include <stdint.h>

uint32_t fn64_synthetic_recompiled_step(uint32_t value) {
    return (value ^ UINT32_C(0x4e363452)) + UINT32_C(0x1020304);
}
