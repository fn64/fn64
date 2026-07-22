#include <stdint.h>

uint32_t fn64_synthetic_section_bridge(uint32_t value) {
    return (value << 7) | (value >> 25);
}
