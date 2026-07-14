//! Overlay/section registry: `get_function`'s backing table.
//!
//! See `docs/DESIGN.md` section 1 and `aki-recomp/runtime/ABI-SURFACE.md`
//! section (d) ("`recomp_overlays.inl` structure") for the exact generated
//! shape this module must consume: one `FuncEntry[]` per section, a
//! `SectionTableEntry[]` naming each section's `(rom_addr, ram_addr, size)`
//! range plus its `FuncEntry` array, and (per `ABI-SURFACE.md`'s own
//! `nm`/completeness-gate observation and this module's own trial-link
//! evidence) `get_function(int32_t vram) -> recomp_func_t*` is the ONE
//! dispatch primitive every `LOOKUP_FUNC` call site in generated C resolves
//! an indirect call through -- 85 call sites in NWXE's corpus alone (per
//! `M1-WORKLIST.md`'s worklist entry #1).
//!
//! ## Why a registry, not a single flat map
//!
//! `recomp_overlays.inl`'s own `SectionTableEntry` table is not one
//! non-overlapping address space: `games/NWXE/RecompiledFuncs/recomp_overlays.inl`
//! (read directly, not the disallowed GPL runtime -- this file is
//! N64Recomp's own MIT-licensed generated output, the ABI this crate
//! serves) declares six sections whose `ram_addr` ranges genuinely
//! *overlap* -- section 2 (`bank1_text`) and section 5 (`bank4_text`) both
//! claim `ram_addr = 0x800E1B90`; section 3 (`bank2_text`) and section 4
//! (`bank3_text`) both claim `ram_addr = 0x8011C900`. This is real N64
//! bank-switched ROM overlay behavior (the cartridge's PI-mapped banks are
//! swapped in and out of the same CPU address window at runtime), not a
//! generation artifact -- a registry that resolved purely by
//! `ram_addr -> function` with no notion of "which section is currently
//! resident" would silently pick whichever bank happened to register last
//! at a shared address, which is exactly the kind of silent-wrong-answer
//! `AGENTS.md`'s "loud traps, no silent shrugs" rule forbids. `SectionRegistry`
//! instead tracks `loaded: HashSet<SectionIndex>` explicitly (mutated by
//! `set_section_loaded`/`set_section_unloaded`, called by whatever ROM/PI
//! DMA logic performs the overlay swap-in on this game) and only resolves a
//! `vram` against a section that is BOTH within-range AND currently marked
//! loaded; section 0 and section 1 (`entry`/`main`, the two non-overlapping,
//! always-resident sections in this corpus) are marked loaded once at
//! registration and never unloaded, matching their role as the
//! always-mapped boot/main code.
//!
//! ## Loud-trap contract
//!
//! A miss (no loaded section's range contains `vram`, or the range contains
//! it but no `FuncEntry` matches the exact offset) panics with the `vram`
//! named in the message -- per `AGENTS.md` and the task's explicit
//! requirement ("loud trap naming the vram on miss"). This is a real,
//! load-bearing correctness boundary: a silent "return a no-op function
//! pointer" here would let a boot sequence "progress" while actually
//! branching to garbage, exactly the failure class `docs/DESIGN.md`'s
//! `osCreateThread_recomp`/`osStartThread_recomp` `unimplemented!()`s were
//! already designed to avoid one layer up.

use std::collections::{HashMap, HashSet};

/// Index into `SectionRegistry`'s registered sections -- mirrors
/// `SectionTableEntry.index` (`ABI-SURFACE.md` section (d)) rather than
/// inventing a second numbering scheme.
pub type SectionIndex = usize;

/// One function's entry in a registered section, mirroring generated
/// `FuncEntry` (`ABI-SURFACE.md` section (d): `func`, `offset`, `rom_size`).
/// `func` is a raw function pointer with the real recompiled ABI shape
/// (`recomp_func_t*` = `extern "C" fn(*mut u8, *mut RecompContext)`, per
/// `ABI-SURFACE.md` section (a)/(b)) -- `fn64-runtime` has no
/// `RecompContext` type of its own (that's `fn64-abi`'s, per
/// `docs/DESIGN.md` section 1's crate split), so this is stored as an
/// opaque `usize` (the raw pointer bits) and only ever handed back out
/// verbatim by `resolve`; `fn64-abi` is the one place that casts it back to
/// a callable function pointer with the real signature.
#[derive(Copy, Clone, Debug)]
pub struct FuncEntry {
    /// Raw `recomp_func_t*` bits, opaque to this crate (see struct doc).
    pub func_ptr: usize,
    /// Offset from the owning section's `ram_addr`.
    pub offset: u32,
    pub rom_size: u32,
}

/// Mirrors generated `SectionTableEntry` (`ABI-SURFACE.md` section (d)):
/// `rom_addr`/`ram_addr`/`size` plus the section's own `FuncEntry` list.
/// `relocs` are declared in the generated shape but have no known call site
/// consuming them yet in either game's corpus as of `ABI-SURFACE.md`'s
/// extraction (every `SectionTableEntry` literal in NWXE's
/// `recomp_overlays.inl` sets `.relocs = nullptr, .num_relocs = 0`) --
/// omitted here rather than modeled speculatively; a future wave that finds
/// a real nonzero-reloc section extends this struct then, with its own
/// citation, per `AGENTS.md`'s "don't guess the shape."
pub struct Section {
    pub rom_addr: u32,
    pub ram_addr: u32,
    pub size: u32,
    pub funcs: Vec<FuncEntry>,
}

impl Section {
    fn contains(&self, vram: u32) -> bool {
        vram >= self.ram_addr && vram < self.ram_addr.wrapping_add(self.size)
    }
}

/// The `get_function` backing store. See module doc for the overlap
/// rationale. Registered once per game at startup (from the game's own
/// `recomp_overlays.inl`-derived data, marshalled by `fn64-abi`/`fn64-shell`
/// -- this crate has no file-parsing or codegen knowledge of its own, only
/// the runtime resolution structure), then queried on every `LOOKUP_FUNC`
/// call site via `resolve`.
#[derive(Default)]
pub struct SectionRegistry {
    sections: Vec<Section>,
    /// Which section indices are currently PI-mapped/resident. Per module
    /// doc: a section not in this set is never considered for resolution,
    /// even if its declared `ram_addr` range would otherwise match --
    /// this is what makes the two genuinely-overlapping bank pairs in
    /// NWXE's corpus resolve correctly rather than by declaration-order
    /// accident.
    loaded: HashSet<SectionIndex>,
    /// Fast path: `vram -> (section, func index)` for already-resolved
    /// addresses, invalidated wholesale on any load/unload (bank switches
    /// are rare relative to `LOOKUP_FUNC` call volume -- 85 call sites in
    /// NWXE's corpus per `M1-WORKLIST.md` -- so a full invalidate on the
    /// rare event, rather than a per-entry staleness check on the hot
    /// path, is the right trade here).
    cache: HashMap<u32, usize>,
}

impl SectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a section. Returns its `SectionIndex` (assigned in
    /// registration order, matching `SectionTableEntry.index`'s own
    /// declared numbering as long as the caller registers in the same
    /// order the generated table declares them -- the caller's
    /// responsibility, not re-derived here, since this crate doesn't parse
    /// the `.inl` itself).
    pub fn register_section(&mut self, section: Section) -> SectionIndex {
        self.sections.push(section);
        self.sections.len() - 1
    }

    /// Mark a section as currently PI-mapped/resident (see module doc).
    /// Idempotent -- marking an already-loaded section loaded again is not
    /// an error (a caller re-asserting current state, not a bug).
    pub fn set_section_loaded(&mut self, index: SectionIndex) {
        assert!(
            index < self.sections.len(),
            "set_section_loaded: no such section index {index}"
        );
        self.loaded.insert(index);
        self.cache.clear();
    }

    /// Mark a section as no longer resident (the corresponding bank has
    /// been PI-swapped out). Any address that only resolved because this
    /// section was loaded must stop resolving -- hence the cache clear,
    /// not a per-entry check.
    pub fn set_section_unloaded(&mut self, index: SectionIndex) {
        self.loaded.remove(&index);
        self.cache.clear();
    }

    pub fn is_section_loaded(&self, index: SectionIndex) -> bool {
        self.loaded.contains(&index)
    }

    /// `get_function(int32_t vram) -> recomp_func_t*`. Per module doc: only
    /// considers sections currently marked loaded, and panics loudly
    /// (naming `vram`) on a miss -- there is no fallback return value,
    /// matching `AGENTS.md`'s "unimplemented ABI surface panics with the
    /// symbol name and call context" for this, the single most-called
    /// dispatch primitive in the whole ABI surface (`ABI-SURFACE.md`
    /// section (a): 85 NWXE / 66 NW4E `LOOKUP_FUNC` call sites).
    pub fn resolve(&mut self, vram: u32) -> usize {
        if let Some(&func_ptr) = self.cache.get(&vram) {
            return func_ptr;
        }
        for &idx in &self.loaded {
            let section = &self.sections[idx];
            if !section.contains(vram) {
                continue;
            }
            let want_offset = vram - section.ram_addr;
            if let Some(entry) = section.funcs.iter().find(|f| f.offset == want_offset) {
                self.cache.insert(vram, entry.func_ptr);
                return entry.func_ptr;
            }
        }
        panic!(
            "get_function: no loaded section resolves vram {vram:#010x} to a function -- either \
             the target address has no FuncEntry at that exact offset in any registered section, \
             or the section that would contain it is not currently marked loaded (see \
             SectionRegistry::set_section_loaded -- an overlay bank must be PI-swapped in before \
             LOOKUP_FUNC calls into it, matching real N64 bank-switch semantics). This is a loud \
             trap per AGENTS.md, not a silent no-op: a missing/wrong function pointer here would \
             let boot 'progress' while branching to garbage."
        );
    }

    pub fn section_count(&self) -> usize {
        self.sections.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(ram_addr: u32, size: u32, funcs: Vec<(u32, usize)>) -> Section {
        Section {
            rom_addr: 0,
            ram_addr,
            size,
            funcs: funcs
                .into_iter()
                .map(|(offset, func_ptr)| FuncEntry {
                    func_ptr,
                    offset,
                    rom_size: 4,
                })
                .collect(),
        }
    }

    #[test]
    fn resolves_a_function_in_an_always_loaded_section() {
        let mut reg = SectionRegistry::new();
        let idx = reg.register_section(section(0x8000_0400, 0x50, vec![(0x0, 0xdead)]));
        reg.set_section_loaded(idx);
        assert_eq!(reg.resolve(0x8000_0400), 0xdead);
    }

    #[test]
    #[should_panic(expected = "get_function")]
    fn unloaded_section_never_resolves_even_if_in_range() {
        let mut reg = SectionRegistry::new();
        let idx = reg.register_section(section(0x8000_0400, 0x50, vec![(0x0, 0xdead)]));
        // Deliberately never loaded.
        let _ = idx;
        reg.resolve(0x8000_0400);
    }

    #[test]
    #[should_panic(expected = "0x80000401")]
    fn miss_panics_naming_the_vram() {
        let mut reg = SectionRegistry::new();
        let idx = reg.register_section(section(0x8000_0400, 0x50, vec![(0x0, 0xdead)]));
        reg.set_section_loaded(idx);
        // In-range but no FuncEntry at offset 1 (unaligned/garbage target).
        reg.resolve(0x8000_0401);
    }

    /// The real, load-bearing overlap case: two sections declaring the SAME
    /// ram_addr range (bank-switch), per module doc's citation of NWXE's
    /// actual recomp_overlays.inl (section 2 and section 5 both claim
    /// 0x800E1B90). Only the currently-loaded one may resolve.
    #[test]
    fn overlapping_bank_sections_resolve_only_the_loaded_one() {
        let mut reg = SectionRegistry::new();
        let bank_a = reg.register_section(section(0x800E_1B90, 0x1000, vec![(0x10, 0xaaaa)]));
        let bank_b = reg.register_section(section(0x800E_1B90, 0x1000, vec![(0x10, 0xbbbb)]));

        reg.set_section_loaded(bank_a);
        assert_eq!(reg.resolve(0x800E_1BA0), 0xaaaa);

        // Bank-switch: swap A out, B in. The cache must not serve A's stale
        // answer for the same vram.
        reg.set_section_unloaded(bank_a);
        reg.set_section_loaded(bank_b);
        assert_eq!(reg.resolve(0x800E_1BA0), 0xbbbb);
    }

    #[test]
    #[should_panic(expected = "get_function")]
    fn after_unloading_the_only_provider_the_address_stops_resolving() {
        let mut reg = SectionRegistry::new();
        let idx = reg.register_section(section(0x8000_0400, 0x50, vec![(0x0, 0xdead)]));
        reg.set_section_loaded(idx);
        assert_eq!(reg.resolve(0x8000_0400), 0xdead);
        reg.set_section_unloaded(idx);
        reg.resolve(0x8000_0400); // must panic now, not serve the cache
    }

    #[test]
    fn resolve_is_idempotent_via_cache() {
        let mut reg = SectionRegistry::new();
        let idx = reg.register_section(section(0x8000_0400, 0x50, vec![(0x0, 0xdead)]));
        reg.set_section_loaded(idx);
        assert_eq!(reg.resolve(0x8000_0400), 0xdead);
        assert_eq!(reg.resolve(0x8000_0400), 0xdead); // cached path
    }
}
