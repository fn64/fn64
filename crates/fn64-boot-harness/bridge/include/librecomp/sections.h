/* Clean-room stand-in for the public generated-section ABI. The generated
 * `recomp_overlays.inl` designated initializers mechanically fix these field
 * names, order, and types; see aki-recomp/runtime/ABI-SURFACE.md section (d).
 * No GPL runtime implementation or header body was consulted.
 */
#ifndef FN64_CLEANROOM_SECTIONS_H
#define FN64_CLEANROOM_SECTIONS_H

#include <stdint.h>
#include <stddef.h>
#include "recomp.h"

#define ARRLEN(x) (sizeof(x) / sizeof((x)[0]))

typedef struct {
    recomp_func_t* func;
    uint32_t offset;
    uint32_t rom_size;
} FuncEntry;

typedef enum {
    R_MIPS_NONE = 0,
    R_MIPS_16,
    R_MIPS_32,
    R_MIPS_REL32,
    R_MIPS_26,
    R_MIPS_HI16,
    R_MIPS_LO16,
    R_MIPS_GPREL16,
} RelocEntryType;

typedef struct {
    uint32_t offset;
    uint32_t target_section_offset;
    uint16_t target_section;
    RelocEntryType type;
} RelocEntry;

typedef struct {
    uint32_t rom_addr;
    uint32_t ram_addr;
    uint32_t size;
    FuncEntry *funcs;
    size_t num_funcs;
    RelocEntry* relocs;
    size_t num_relocs;
    size_t index;
} SectionTableEntry;

#endif
