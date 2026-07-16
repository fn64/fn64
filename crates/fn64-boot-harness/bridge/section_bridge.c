/* Shared bridge from N64Recomp's generated `recomp_overlays.inl` to the
 * game-agnostic Rust boot harness. Game-derived tables and functions are
 * supplied out-of-tree through RECOMPILED_DIR and never copied into fn64.
 *
 * fn64-runtime's SectionRegistry deliberately has no generated-file parsing
 * knowledge. This translation unit is the one adapter that knows the public
 * SectionTableEntry/FuncEntry shape mechanically documented in
 * aki-recomp/runtime/ABI-SURFACE.md section (d).
 */
#include <stdint.h>
#include <stddef.h>
#include "recomp.h"
#include "funcs.h"

/* The generated include names librecomp/sections.h. Build scripts place this
 * crate's clean-room stand-in ahead of any other include directory. */
#include "recomp_overlays.inl"

/* Generated initializers use C++ `nullptr`, so this file is compiled as C++.
 * Keep the bridge surface unmangled for Rust's extern "C" declarations. */
#ifdef __cplusplus
extern "C" {
#endif

extern void fn64_register_func(
    size_t section_index,
    uint32_t rom_addr,
    uint32_t ram_addr,
    uint32_t size,
    uint32_t offset,
    uint32_t rom_size,
    recomp_func_t* func
);

void fn64_bridge_register_all_sections(void) {
    for (size_t s = 0; s < num_sections; s++) {
        SectionTableEntry* entry = &section_table[s];
        for (size_t f = 0; f < entry->num_funcs; f++) {
            FuncEntry* fe = &entry->funcs[f];
            fn64_register_func(
                entry->index,
                entry->rom_addr,
                entry->ram_addr,
                entry->size,
                fe->offset,
                fe->rom_size,
                fe->func
            );
        }
    }
}

size_t fn64_bridge_num_sections(void) {
    return num_sections;
}

#ifdef __cplusplus
}
#endif
