/* Clean-room stand-in for N64ModernRuntime's `librecomp/include/librecomp/
 * sections.h`, which `recomp_overlays.inl` (N64Recomp's own MIT-licensed
 * generated output) `#include`s directly. The REAL header lives under
 * N64ModernRuntime's GPL-3.0-licensed tree (that repo's top-level COPYING
 * is GPL-3.0; `librecomp/` is not under the MIT-carved-out `N64Recomp/`
 * subdirectory), which fn64/AGENTS.md's clean-room protocol disallows
 * linking/including verbatim.
 *
 * This header is hand-written from the PUBLIC, mechanically-documented ABI
 * shape only (the exact same provenance `fn64/crates/fn64-runtime/src/
 * overlay.rs`'s module doc already cites for the identical struct layout):
 * `SectionTableEntry`/`FuncEntry`/`RelocEntry`'s field names and order are
 * fixed by `recomp_overlays.inl`'s own designated-initializer syntax
 * (`.rom_addr = ..., .ram_addr = ..., .size = ..., .funcs = ..., .num_funcs
 * = ..., .relocs = nullptr, .num_relocs = 0, .index = ...` -- verified
 * directly against the real generated file, which is allowed source per
 * AGENTS.md: "the MIT recompiler source... and the C it generates -- that's
 * the ABI we serve"), not copied from the GPL header's implementation.
 *
 * Placed on the include path AHEAD of any real librecomp/include dir so the
 * preprocessor resolves `#include "librecomp/sections.h"` to THIS file,
 * never the GPL one -- see ../../build.rs.
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
