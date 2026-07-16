/* section_bridge.c -- glues the out-of-tree, game-derived
 * `recomp_overlays.inl` (N64Recomp's own MIT-licensed generated output,
 * compiled here from RECOMPILED_DIR, never checked into fn64 -- see
 * fn64/README.md's "no game content ships in this repo" rule and
 * fn64/AGENTS.md's clean-room protocol) to a small, hand-written,
 * game-agnostic C surface Rust can call via FFI.
 *
 * fn64-runtime's SectionRegistry (crates/fn64-runtime/src/overlay.rs) has no
 * knowledge of `recomp_overlays.inl`'s file format at all, by design
 * (docs/DESIGN.md section 1: "this crate has no file-parsing or codegen
 * knowledge of its own"). This file is that missing piece, living OUTSIDE
 * the fn64 crates (in this example harness only) precisely because it is
 * the one piece of glue that DOES need to know the generated shape -- it
 * contains zero game logic itself, only the `SectionTableEntry[]`/
 * `FuncEntry[]` walk, so it stays honest to "fn64 has no game-specific
 * code" even though this .c file lives in the fn64 repo (it contains no
 * ROM bytes, no recompiled function bodies -- see AGENTS.md's "no ROM
 * bytes... ever enters git": this file is pure glue, checked in; the DATA
 * it walks (recomp_overlays.inl, funcs_*.c) is supplied at build time from
 * RECOMPILED_DIR and never copied into this repository).
 */
#include <stdint.h>
#include <stddef.h>
#include "recomp.h"
#include "funcs.h"

/* recomp_overlays.inl itself #includes "librecomp/sections.h" -- resolved
 * to this harness's OWN clean-room stand-in header (bridge/include/
 * librecomp/sections.h), placed ahead of any real librecomp/include dir on
 * the include path by build.rs, never the GPL-3.0-licensed real one. See
 * that header's doc comment for the full provenance/rationale. */
#include "recomp_overlays.inl"

/* This translation unit is compiled as C++ (build.rs), NOT C, because
 * recomp_overlays.inl's SectionTableEntry initializers use `nullptr`
 * (N64Recomp generates this file targeting a C++ port build) -- everything
 * ABOVE this point (recomp.h, funcs.h, recomp_overlays.inl, RecompiledFuncs
 * itself) is otherwise plain, portable C; only this bridging boundary needs
 * to be aware of the C++ compile mode, via explicit `extern "C"` linkage so
 * Rust's #[no_mangle] FFI declarations (which assume plain C symbol names)
 * still resolve correctly. */
#ifdef __cplusplus
extern "C" {
#endif

/* Implemented in Rust (examples/wm2000-boot/src/main.rs's register_section
 * FFI wrapper) -- called once per FuncEntry in registration order, matching
 * SectionTableEntry.index's own numbering (fn64_runtime::overlay's
 * documented registration-order contract). */
extern void fn64_register_func(
    size_t section_index,
    uint32_t rom_addr,
    uint32_t ram_addr,
    uint32_t size,
    uint32_t offset,
    uint32_t rom_size,
    recomp_func_t* func
);

/* Called once by the harness after linking, before running boot. Walks the
 * REAL generated section_table[]/FuncEntry[] arrays (compiled into this
 * translation unit from the game's own recomp_overlays.inl) and hands every
 * (section, func) pair to Rust's fn64_register_func. */
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
