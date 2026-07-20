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

/// Pointer-independent function metadata retained by release evidence.
///
/// Function order is preserved because [`SectionRegistry::resolve`] selects
/// the first exact offset in a section. `func_ptr` is deliberately absent:
/// native address bits vary by process and do not identify the callable body.
/// The program-owning layer must bind callable identity separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuncEntryEvidenceSnapshot {
    pub offset: u32,
    pub rom_size: u32,
}

/// Registration-order section geometry and function metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionEvidenceSnapshot {
    pub rom_addr: u32,
    pub ram_addr: u32,
    pub size: u32,
    pub funcs: Vec<FuncEntryEvidenceSnapshot>,
}

/// One runtime-relocated section base, canonicalized by section index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionLoadEvidenceSnapshot {
    pub section: SectionIndex,
    pub load_vram: u32,
}

/// Furthest committed static-image byte for one section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticStorageEndEvidenceSnapshot {
    pub section: SectionIndex,
    pub end: u32,
}

/// Exact in-flight cursor for a chunked static-image mirror.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMirrorEvidenceSnapshot {
    pub section: SectionIndex,
    pub next_rom: u32,
    pub next_static_off: u32,
}

/// Canonical, future-affecting view of the section registry.
///
/// Registration order and per-section function order remain semantic and are
/// retained. Hash-backed collections are sorted by section index so allocator
/// seeds and equivalent insertion histories cannot perturb evidence. The
/// derived lookup cache and process-specific function pointers are excluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionRegistryEvidenceSnapshot {
    pub sections: Vec<SectionEvidenceSnapshot>,
    pub loaded_sections: Vec<SectionIndex>,
    pub runtime_loads: Vec<SectionLoadEvidenceSnapshot>,
    pub static_mirror: Option<StaticMirrorEvidenceSnapshot>,
    pub static_storage_ends: Vec<StaticStorageEndEvidenceSnapshot>,
}

impl Section {
    /// Range check against an explicit load base -- `base` is the section's
    /// static `ram_addr` for resident/fixed-vram sections, or its runtime
    /// DMA-relocated heap base for sections in `load_vram` (see that field's
    /// doc). Offsets in `funcs` are relative to whichever base is in effect.
    fn contains_at(&self, base: u32, vram: u32) -> bool {
        vram >= base && vram < base.wrapping_add(self.size)
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
    /// Runtime load base for sections the game DMA-loaded and relocated to a
    /// heap address that differs from the section's static link-time
    /// `ram_addr`. OoT's gamestate/actor overlays are DMA'd from ROM to a
    /// SystemArena allocation, then `Overlay_Relocate` rewrites their
    /// absolute pointers by `+(loadedRamAddr - vramStart)` -- so a
    /// `LOOKUP_FUNC` after that load arrives with a *heap* vram, not the
    /// static `ram_addr`. When an index is present here, `resolve` treats
    /// this value (not `ram_addr`) as the section's base for both range and
    /// per-func offset math. Absent means "resolve at the static
    /// `ram_addr`" -- the resident-image sections (0/1/2) and the AKI
    /// fixed-vram bank-swap overlays, which are not runtime-relocated.
    /// (`recomp_overlays.inl` declares `num_relocs=0` for every section in
    /// this build: the section-table reloc field models a *different*
    /// mechanism than the game's own `Overlay_Relocate`, so we key off the
    /// game's runtime DMA destination, not the static table.)
    load_vram: HashMap<SectionIndex, u32>,
    /// Fast path: `vram -> (section, func index)` for already-resolved
    /// addresses, invalidated wholesale on any load/unload (bank switches
    /// are rare relative to `LOOKUP_FUNC` call volume -- 85 call sites in
    /// NWXE's corpus per `M1-WORKLIST.md` -- so a full invalidate on the
    /// rare event, rather than a per-entry staleness check on the hot
    /// path, is the right trade here).
    cache: HashMap<u32, usize>,
    /// One in-flight static-link-VRAM mirror (see `plan_static_mirror`). An
    /// overlay's chunked DMA is mirrored to its static link VRAM so the
    /// baked static-address DATA reads in this fully-static build resolve.
    static_mirror: Option<StaticMirror>,
    /// Furthest byte actually mirrored for each static overlay image. Section
    /// geometry describes recompiled text, while an overlay ROM image also
    /// contains data referenced by baked absolute pointers.
    static_storage_ends: HashMap<SectionIndex, u32>,
}

/// In-flight state for `plan_static_mirror`: the ROM cursor (where the next
/// contiguous chunk must start to continue this mirror) and the matching
/// rdram static-VRAM destination cursor.
#[derive(Copy, Clone)]
struct StaticMirror {
    section: SectionIndex,
    next_rom: u32,
    next_static_off: u32,
}

impl SectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Capture every modeled registry field that can change later overlay
    /// resolution or static-image mirroring, without binding native pointers.
    pub fn evidence_snapshot(&self) -> SectionRegistryEvidenceSnapshot {
        let sections = self
            .sections
            .iter()
            .map(|section| SectionEvidenceSnapshot {
                rom_addr: section.rom_addr,
                ram_addr: section.ram_addr,
                size: section.size,
                funcs: section
                    .funcs
                    .iter()
                    .map(|entry| FuncEntryEvidenceSnapshot {
                        offset: entry.offset,
                        rom_size: entry.rom_size,
                    })
                    .collect(),
            })
            .collect();

        let mut loaded_sections: Vec<_> = self.loaded.iter().copied().collect();
        loaded_sections.sort_unstable();

        let mut runtime_loads: Vec<_> = self
            .load_vram
            .iter()
            .map(|(&section, &load_vram)| SectionLoadEvidenceSnapshot { section, load_vram })
            .collect();
        runtime_loads.sort_unstable_by_key(|load| load.section);

        let mut static_storage_ends: Vec<_> = self
            .static_storage_ends
            .iter()
            .map(|(&section, &end)| StaticStorageEndEvidenceSnapshot { section, end })
            .collect();
        static_storage_ends.sort_unstable_by_key(|storage| storage.section);

        SectionRegistryEvidenceSnapshot {
            sections,
            loaded_sections,
            runtime_loads,
            static_mirror: self
                .static_mirror
                .map(|mirror| StaticMirrorEvidenceSnapshot {
                    section: mirror.section,
                    next_rom: mirror.next_rom,
                    next_static_off: mirror.next_static_off,
                }),
            static_storage_ends,
        }
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
        // A static-vram load supersedes any prior runtime-relocated base for
        // this same index (resolve at `ram_addr` again).
        self.load_vram.remove(&index);
        self.loaded.insert(index);
        self.cache.clear();
    }

    /// Mark a section loaded at a *runtime* base `load_vram` -- the heap vram
    /// the game DMA'd it to and relocated its runtime data pointers against,
    /// distinct from the section's static `ram_addr`. Used by the DMA-driven
    /// overlay-load path (`load_section_at_rom_addr`): after this, `resolve`
    /// maps addresses in `[load_vram, load_vram+size)` (relocated data
    /// pointers) AND in `[ram_addr, ram_addr+size)` (static code immediates
    /// the recompiler didn't relocate) through this section's funcs, so a
    /// `LOOKUP_FUNC` for either flavor of pointer resolves correctly (see
    /// `resolve`'s two-base loop for the citation of why both occur).
    pub fn set_section_loaded_at(&mut self, index: SectionIndex, load_vram: u32) {
        assert!(
            index < self.sections.len(),
            "set_section_loaded_at: no such section index {index}"
        );
        // A runtime byte can hold only one code image. OoT reuses arena spans
        // at different alignments as well as identical bases: after the
        // Link's House -> Kokiri transition, the new KiraKira callback was
        // canonicalized through a stale, partially-overlapping effect span to
        // 0x80B2CBB0 (inside EffectSsDust_Draw) instead of the exact
        // EffectSsKiraKira_Init entry at 0x80B2D1B0. Evict every overlapping
        // runtime range before publishing the new image so HashSet iteration
        // order cannot select an obsolete translation. OoT NTSC 1.0 ROM
        // 0x00EA82E0 contains 0x27A40134 at the wrong Dust interior PC, while
        // KiraKira's exact ROM entry 0x00EA88E0 contains 0xAFA40000.
        let new_end = u64::from(load_vram) + u64::from(self.sections[index].size);
        let displaced: Vec<_> = self
            .load_vram
            .iter()
            .filter_map(|(&other, &base)| {
                let old_end = u64::from(base) + u64::from(self.sections[other].size);
                (other != index && u64::from(base) < new_end && u64::from(load_vram) < old_end)
                    .then_some(other)
            })
            .collect();
        for other in displaced {
            self.load_vram.remove(&other);
            self.loaded.remove(&other);
        }
        self.load_vram.insert(index, load_vram);
        self.loaded.insert(index);
        self.cache.clear();
    }

    /// Honor a game-driven overlay DMA: given the ROM source address and the
    /// RDRAM/vram destination of a DMA the game just performed, if some
    /// registered section's `rom_addr` matches `rom_addr` exactly, mark that
    /// section loaded at `dest_vram` and return its index. Returns `None`
    /// (a no-op) for any DMA that is not an overlay-section load -- ordinary
    /// data DMAs (dmadata table, object files, audio banks) must NOT be
    /// mistaken for code-section loads, so the match is on the exact
    /// section-start `rom_addr`, never a range containment.
    ///
    /// This is the general overlay-load hook: it keys off the game's own DMA
    /// action rather than the harness hard-coding which overlays to load, so
    /// every overlay the game DMAs in (gamestate overlays, actor overlays)
    /// becomes resolvable automatically at its true runtime base. Sections
    /// the game never DMAs stay unloaded, keeping `resolve`'s loud trap live
    /// for genuinely-absent code.
    pub fn load_section_at_rom_addr(
        &mut self,
        rom_addr: u32,
        dest_vram: u32,
    ) -> Option<SectionIndex> {
        let idx = self.sections.iter().position(|s| s.rom_addr == rom_addr)?;
        self.set_section_loaded_at(idx, dest_vram);
        Some(idx)
    }

    /// Plan a static-link-VRAM mirror write for one DMA chunk.
    ///
    /// This corpus is a FULLY STATIC N64Recomp build: `recomp_overlays.inl`
    /// carries `num_relocs = 0` for every section and the generated C emits
    /// NO `RELOC_HI16`/`RELOC_LO16` macros -- every address is a baked
    /// absolute link-time literal (verified: `grep RELOC_HI16 games/OOTU/
    /// RecompiledFuncs/*.c` is empty). So when an overlay's recompiled code
    /// reads its own DATA -- e.g. `Player_InitItemAction` does
    /// `lui $t9,0x8085; lw $t9,0x1EA8($t9)` (funcs_67.c) to fetch
    /// `sItemActionInitFuncs[]` -- it dereferences the *static link VRAM*
    /// `0x80851EA8`, NOT the runtime heap address the game DMA'd the overlay
    /// to. But OoT's own loader DMAs the overlay to an arena HEAP base
    /// (`Actor_Spawn`/`KaleidoManager_LoadOvl` -> `Overlay_Load`) and the
    /// game's `Overlay_Relocate` rewrites the *heap* copy's pointers; the
    /// static-link-VRAM region is left unwritten (reads 0) -> the baked
    /// `lw` reads a NULL function pointer -> `LOOKUP_FUNC(0)` traps.
    ///
    /// A static build requires the overlay's image to be resident at its
    /// static link VRAM, holding the RAW (un-relocated) link-time pointers
    /// (e.g. `sItemActionInitFuncs[0] = 0x808317A4 = Player_InitDefaultIA`).
    /// `resolve` already accepts a section's static `ram_addr` as a base
    /// (base #2, for baked code immediates), so a raw static pointer read
    /// from the mirror resolves through the very same FuncEntry list. The
    /// game's separate heap copy + `Overlay_Relocate` still run untouched
    /// (faithful) -- fn64 just additionally makes the static-VRAM image the
    /// baked reads target actually present.
    ///
    /// The game DMAs an overlay contiguously in chunks (OoT's DmaMgr splits
    /// into `0x2000` blocks). This tracks one active mirror at a time: it
    /// STARTS when a chunk's ROM source is exactly a section's `rom_addr`,
    /// and CONTINUES for each subsequent chunk whose ROM source is exactly
    /// where the previous chunk ended (contiguous). A non-contiguous chunk
    /// (an unrelated DMA, or the next overlay) ends the active mirror, so an
    /// unrelated data DMA following the overlay is never mis-mirrored past
    /// the overlay's own ROM extent. Returns the rdram DESTINATION offset the
    /// caller must ALSO write this chunk's (already byte-swizzled) bytes to,
    /// or `None` if this chunk is not part of an active static-overlay load.
    pub fn plan_static_mirror(&mut self, dev_addr: u32, len: u32) -> Option<u32> {
        // Start a new mirror if this chunk begins exactly at a section start.
        if let Some(idx) = self.sections.iter().position(|s| s.rom_addr == dev_addr) {
            let ram_addr = self.sections[idx].ram_addr;
            // Static link VRAM -> rdram offset (KSEG0 mask). The Player
            // overlay's DATA sits above physical 8MB RDRAM (link 0x808301C0+);
            // the caller's rdram buffer is oversized to cover it (see
            // fn64-runtime::Rdram::new_with_mmio / oot-boot's rdram sizing).
            let static_off = ram_addr & 0x1FFF_FFFF;
            let mirrored_end = static_off
                .checked_add(len)
                .expect("static overlay mirror range overflow");
            self.static_storage_ends
                .entry(idx)
                .and_modify(|end| *end = (*end).max(mirrored_end))
                .or_insert(mirrored_end);
            self.static_mirror = Some(StaticMirror {
                section: idx,
                next_rom: dev_addr.wrapping_add(len),
                next_static_off: static_off.wrapping_add(len),
            });
            return Some(static_off);
        }
        // Continue an active mirror only for the exact contiguous next chunk.
        if let Some(m) = self.static_mirror {
            if m.next_rom == dev_addr {
                let dest = m.next_static_off;
                let mirrored_end = dest
                    .checked_add(len)
                    .expect("static overlay mirror continuation range overflow");
                self.static_storage_ends
                    .entry(m.section)
                    .and_modify(|end| *end = (*end).max(mirrored_end))
                    .or_insert(mirrored_end);
                self.static_mirror = Some(StaticMirror {
                    section: m.section,
                    next_rom: dev_addr.wrapping_add(len),
                    next_static_off: dest.wrapping_add(len),
                });
                return Some(dest);
            }
        }
        // Any non-contiguous chunk ends the active mirror.
        self.static_mirror = None;
        None
    }

    /// Mark a section as no longer resident (the corresponding bank has
    /// been PI-swapped out). Any address that only resolved because this
    /// section was loaded must stop resolving -- hence the cache clear,
    /// not a per-entry check.
    pub fn set_section_unloaded(&mut self, index: SectionIndex) {
        self.loaded.remove(&index);
        self.load_vram.remove(&index);
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
            // A DMA-relocated overlay is reachable at TWO bases, because the
            // recompiler and the game disagree on where its functions live:
            //   1. the runtime heap base the game DMA'd it to -- the value
            //      the game's own `Overlay_Relocate` rewrites its *runtime
            //      data* pointers to (e.g. `gGameStateOverlayTable[].init`,
            //      which arrives at LOOKUP_FUNC as `0x803b4df0`); and
            //   2. the static link-time `ram_addr` -- what the recompiler
            //      baked as *code immediates* inside the overlay's own funcs
            //      (e.g. ConsoleLogo_Init does `lui 0x8080; addiu 0x690` to
            //      store `gameState->main = 0x80800690`; the recompiler does
            //      NOT relocate code immediates, so this static vram also
            //      reaches LOOKUP_FUNC).
            // Both must resolve to the same FuncEntry list. Resident/fixed-
            // vram sections (no `load_vram` entry) have only base #2.
            let static_base = section.ram_addr;
            let bases = [self.load_vram.get(&idx).copied(), Some(static_base)];
            for base in bases.into_iter().flatten() {
                if !section.contains_at(base, vram) {
                    continue;
                }
                let want_offset = vram - base;
                if let Some(entry) = section.funcs.iter().find(|f| f.offset == want_offset) {
                    self.cache.insert(vram, entry.func_ptr);
                    return entry.func_ptr;
                }
            }
        }
        let context = format!(
            "get_function: no loaded section resolves vram {vram:#010x} to a function -- either \
             the target address has no FuncEntry at that exact offset in any registered section, \
             or the section that would contain it is not currently marked loaded (see \
             SectionRegistry::set_section_loaded -- an overlay bank must be PI-swapped in before \
             LOOKUP_FUNC calls into it, matching real N64 bank-switch semantics). This is a loud \
             trap per AGENTS.md, not a silent no-op: a missing/wrong function pointer here would \
             let boot 'progress' while branching to garbage."
        );
        crate::record_unsupported_event(
            crate::UnsupportedSubsystem::Runtime,
            "runtime.overlay.get-function",
            &context,
            None,
            crate::UnsupportedDisposition::LoudTrap,
        );
        panic!("{context}");
    }

    /// Translate a loaded overlay's relocated heap address back to its
    /// static link-time vram. Typed recompiled modules key their function table
    /// by that canonical vram, while the game's relocation pass stores heap
    /// addresses in callback tables. Static/resident addresses return `None`
    /// so a host-first lookup cannot recurse on an unchanged address.
    pub fn canonical_vram(&self, vram: u32) -> Option<u32> {
        for &idx in &self.loaded {
            let section = &self.sections[idx];
            let Some(load_base) = self.load_vram.get(&idx).copied() else {
                continue;
            };
            if load_base != section.ram_addr && section.contains_at(load_base, vram) {
                return Some(section.ram_addr.wrapping_add(vram - load_base));
            }
        }
        None
    }

    /// Host-storage ranges which represent the static-link image of every
    /// loaded section. Fully static recompiled code retains absolute overlay
    /// data addresses, so device adapters may admit these explicit aliases in
    /// addition to physical RDRAM without treating the rest of the oversized
    /// CPU backing allocation as hardware-visible memory.
    pub fn loaded_static_storage_ranges(&self) -> Vec<std::ops::Range<u32>> {
        let mut ranges: Vec<_> = self
            .loaded
            .iter()
            .map(|&idx| {
                let section = &self.sections[idx];
                let start = section.ram_addr & 0x1fff_ffff;
                let text_end = start.checked_add(section.size).unwrap_or_else(|| {
                    panic!(
                        "loaded section {idx} static storage range overflows: start {start:#x}, \
                         size {:#x}",
                        section.size
                    )
                });
                let end = self
                    .static_storage_ends
                    .get(&idx)
                    .copied()
                    .unwrap_or(text_end)
                    .max(text_end);
                start..end
            })
            .collect();
        ranges.sort_unstable_by_key(|range| (range.start, range.end));
        ranges
    }

    pub fn section_count(&self) -> usize {
        self.sections.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section_at_rom(
        rom_addr: u32,
        ram_addr: u32,
        size: u32,
        funcs: Vec<(u32, usize)>,
    ) -> Section {
        Section {
            rom_addr,
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

    /// Address-keyed overlay load: a DMA from a section's exact ROM start to
    /// a heap vram makes that section resolvable at the DMA destination base.
    /// Models OoT's ovl_title gamestate load (rom 0x00b9da40 -> heap
    /// 0x803b4640, ConsoleLogo_Init at static offset 0x7b0 -> relocated
    /// 0x803b4df0). Distinguishable func ptrs so a wrong-offset or
    /// wrong-section resolution is caught, not a coincidental 0.
    #[test]
    fn dma_load_resolves_relocated_pointer_at_heap_base() {
        let mut reg = SectionRegistry::new();
        // Static link-time ram_addr 0x80800000, but DMA'd to heap 0x803b4640.
        let idx = reg.register_section(section_at_rom(
            0x00b9_da40,
            0x8080_0000,
            0x910,
            vec![(0x7b0, 0xc0de_1234)],
        ));
        // Data DMAs to unrelated ROM offsets must NOT load this section.
        assert_eq!(reg.load_section_at_rom_addr(0x0000_1000, 0x8020_0000), None);
        assert!(!reg.is_section_loaded(idx));

        // The game DMAs the overlay from its exact ROM start to a heap addr.
        assert_eq!(
            reg.load_section_at_rom_addr(0x00b9_da40, 0x803b_4640),
            Some(idx)
        );
        // The game's Overlay_Relocate produced init = 0x803b4640 + 0x7b0.
        assert_eq!(reg.canonical_vram(0x803b_4df0), Some(0x8080_07b0));
        assert_eq!(reg.canonical_vram(0x8080_07b0), None);
        assert_eq!(reg.resolve(0x803b_4df0), 0xc0de_1234);
    }

    #[test]
    fn loaded_static_storage_ranges_are_deterministic_and_exclude_unloaded_sections() {
        let mut reg = SectionRegistry::new();
        let high = reg.register_section(section_at_rom(0x2000, 0x8080_0000, 0x1000, vec![]));
        let low = reg.register_section(section_at_rom(0x1000, 0x8000_0400, 0x200, vec![]));
        reg.set_section_loaded_at(high, 0x803b_4000);
        reg.set_section_loaded(low);

        assert_eq!(
            reg.loaded_static_storage_ranges(),
            vec![0x400..0x600, 0x80_0000..0x80_1000]
        );
        reg.set_section_unloaded(high);
        assert_eq!(reg.loaded_static_storage_ranges(), vec![0x400..0x600]);
    }

    #[test]
    fn loaded_static_storage_range_includes_mirrored_overlay_data_after_text() {
        let mut reg = SectionRegistry::new();
        let title = reg.register_section(section_at_rom(0x00b9_da40, 0x8080_0000, 0x910, vec![]));

        assert_eq!(reg.plan_static_mirror(0x00b9_da40, 0x9c0), Some(0x80_0000));
        reg.set_section_loaded_at(title, 0x803b_4640);
        assert_eq!(
            reg.loaded_static_storage_ranges(),
            vec![0x80_0000..0x80_09c0]
        );
    }

    /// OoT's pause and player overlays reuse the same Kaleido arena. Loading
    /// player at the former pause destination must replace pause atomically;
    /// otherwise the relocated Player_Init pointer canonicalizes to an
    /// interior pause PC because both section ranges contain the address.
    #[test]
    fn shared_runtime_base_replaces_prior_overlay() {
        let mut reg = SectionRegistry::new();
        let pause = reg.register_section(section_at_rom(0x00bb_11e0, 0x8081_37c0, 0x15b90, vec![]));
        let player = reg.register_section(section_at_rom(
            0x00bc_db70,
            0x8083_01c0,
            0x21110,
            vec![(0x14c28, 0xfeed_face)],
        ));
        let arena = 0x8038_8b60;

        reg.set_section_loaded_at(pause, arena);
        reg.set_section_loaded_at(player, arena);

        assert!(!reg.is_section_loaded(pause), "displaced image stays stale");
        assert!(reg.is_section_loaded(player));
        assert_eq!(reg.canonical_vram(arena + 0x14c28), Some(0x8084_4de8));
        assert_eq!(reg.resolve(arena + 0x14c28), 0xfeed_face);
    }

    /// OoT's scene teardown/re-init can reuse only part of an arena span at
    /// a different aligned base. The prior exact-base eviction left the old
    /// mapping loaded, so a callback in the overlap canonicalized according
    /// to whichever HashSet entry happened to be visited first.
    #[test]
    fn partially_overlapping_runtime_range_replaces_prior_overlay() {
        let mut reg = SectionRegistry::new();
        let stale = reg.register_section(section_at_rom(
            0x00ea_80b0,
            0x80b2_c980,
            0x740,
            vec![(0x1b4, 0xd057_d057)],
        ));
        let current = reg.register_section(section_at_rom(
            0x00ea_88e0,
            0x80b2_d1b0,
            0x5c0,
            vec![(0, 0xcafe_cafe)],
        ));

        reg.set_section_loaded_at(stale, 0x801d_a000);
        reg.set_section_loaded_at(current, 0x801d_a700);

        assert!(
            !reg.is_section_loaded(stale),
            "a stale image whose tail overlaps the new allocation must be evicted"
        );
        assert!(reg.is_section_loaded(current));
        assert_eq!(reg.canonical_vram(0x801d_a700), Some(0x80b2_d1b0));
        assert_eq!(reg.resolve(0x801d_a700), 0xcafe_cafe);
    }

    /// After a DMA load, the SAME section must ALSO resolve at its static
    /// link-time ram_addr, because the recompiler bakes static vram as code
    /// immediates (ConsoleLogo_Init stores `gameState->main = 0x80800690`
    /// via `lui 0x8080; addiu 0x690`, NOT the relocated heap address). Both
    /// bases hit the same FuncEntry list. This is the exact second trap the
    /// heap-only model would (and did) hit at 0x80800690.
    #[test]
    fn dma_loaded_section_also_resolves_static_code_immediate_base() {
        let mut reg = SectionRegistry::new();
        let idx = reg.register_section(section_at_rom(
            0x00b9_da40,
            0x8080_0000,
            0x910,
            // ConsoleLogo_Init@0x7b0, ConsoleLogo_Main@0x690 -- distinct ptrs.
            vec![(0x7b0, 0xc0de_1234), (0x690, 0x0bad_c0de)],
        ));
        reg.load_section_at_rom_addr(0x00b9_da40, 0x803b_4640)
            .unwrap();
        let _ = idx;
        // Relocated heap pointer (from the runtime GameStateOverlay table).
        assert_eq!(reg.resolve(0x803b_4df0), 0xc0de_1234);
        // Static code-immediate pointer (baked by the recompiler).
        assert_eq!(reg.resolve(0x8080_0690), 0x0bad_c0de);
    }

    /// Unloading a DMA-relocated section drops BOTH its heap and static
    /// bases -- neither may keep resolving from a stale cache/load_vram.
    #[test]
    #[should_panic(expected = "0x803b4df0")]
    fn unloading_dma_section_stops_heap_base_resolution() {
        let mut reg = SectionRegistry::new();
        let idx = reg.register_section(section_at_rom(
            0x00b9_da40,
            0x8080_0000,
            0x910,
            vec![(0x7b0, 0xc0de_1234)],
        ));
        reg.load_section_at_rom_addr(0x00b9_da40, 0x803b_4640)
            .unwrap();
        assert_eq!(reg.resolve(0x803b_4df0), 0xc0de_1234);
        reg.set_section_unloaded(idx);
        reg.resolve(0x803b_4df0); // must panic, not serve stale heap base
    }

    /// `plan_static_mirror` starts a mirror only at an exact section-start
    /// chunk, then continues for exactly-contiguous chunks, and ends the
    /// mirror at any non-contiguous chunk. Models the Player overlay's chunked
    /// DMA (rom 0x00BCDB70, static VRAM 0x808301C0, 0x2000-byte chunks). The
    /// returned static offsets are the KSEG0-masked static VRAM + accumulated
    /// chunk length -- distinguishable so a wrong base/cursor is caught.
    #[test]
    fn plan_static_mirror_tracks_a_contiguous_chunked_overlay_load() {
        let mut reg = SectionRegistry::new();
        // Section 9 geometry. rom 0x00BCDB70, static VRAM 0x808301C0.
        reg.register_section(section_at_rom(0x00bc_db70, 0x8083_01c0, 0x2_1110, vec![]));
        // A second, unrelated section start -- must NOT be mistaken for a
        // continuation of the first mirror.
        reg.register_section(section_at_rom(0x00bf_ac30, 0x8085_d350, 0x4ef0, vec![]));

        let static_base = 0x0083_01c0u32; // 0x808301C0 & 0x1FFFFFFF

        // A data DMA before the overlay must not start a mirror.
        assert_eq!(reg.plan_static_mirror(0x0010_0000, 0x100), None);

        // First chunk at the exact section start -> mirror to static base.
        assert_eq!(
            reg.plan_static_mirror(0x00bc_db70, 0x2000),
            Some(static_base)
        );
        // Contiguous next chunk -> static base + 0x2000.
        assert_eq!(
            reg.plan_static_mirror(0x00bc_fb70, 0x2000),
            Some(static_base + 0x2000)
        );
        // Contiguous again -> + 0x4000.
        assert_eq!(
            reg.plan_static_mirror(0x00bd_1b70, 0x2000),
            Some(static_base + 0x4000)
        );

        // A non-contiguous chunk (an unrelated DMA) ends the mirror: no dest.
        assert_eq!(reg.plan_static_mirror(0x0020_0000, 0x40), None);
        // ...and the previously-active mirror does NOT resume on the next
        // would-be-contiguous address (it was torn down).
        assert_eq!(reg.plan_static_mirror(0x00bd_3b70, 0x2000), None);

        // A fresh section-start begins a brand-new mirror at ITS static base.
        assert_eq!(
            reg.plan_static_mirror(0x00bf_ac30, 0x1000),
            Some(0x0085_d350)
        );
    }

    fn evidence_section(
        rom_addr: u32,
        ram_addr: u32,
        size: u32,
        funcs: &[(u32, u32, usize)],
    ) -> Section {
        Section {
            rom_addr,
            ram_addr,
            size,
            funcs: funcs
                .iter()
                .map(|&(offset, rom_size, func_ptr)| FuncEntry {
                    func_ptr,
                    offset,
                    rom_size,
                })
                .collect(),
        }
    }

    #[test]
    fn evidence_snapshot_is_cache_warm_equivalent_and_pointer_independent() {
        let mut cold = SectionRegistry::new();
        let cold_idx = cold.register_section(evidence_section(
            0x1000,
            0x8000_0400,
            0x80,
            &[(0x10, 0x20, 0x1111)],
        ));
        cold.set_section_loaded(cold_idx);
        let before_resolve = cold.evidence_snapshot();
        assert_eq!(cold.resolve(0x8000_0410), 0x1111);
        assert_eq!(cold.evidence_snapshot(), before_resolve);

        let mut different_native_pointer = SectionRegistry::new();
        let pointer_idx = different_native_pointer.register_section(evidence_section(
            0x1000,
            0x8000_0400,
            0x80,
            &[(0x10, 0x20, 0xeeee)],
        ));
        different_native_pointer.set_section_loaded(pointer_idx);
        assert_eq!(
            different_native_pointer.evidence_snapshot(),
            before_resolve,
            "native function addresses do not identify callable body semantics"
        );
    }

    #[test]
    fn evidence_snapshot_is_hash_insertion_order_independent() {
        fn registry(reverse: bool) -> SectionRegistry {
            let mut reg = SectionRegistry::new();
            let first = reg.register_section(evidence_section(
                0x1000,
                0x8080_0000,
                0x100,
                &[(0, 4, 0x1111)],
            ));
            let second = reg.register_section(evidence_section(
                0x2000,
                0x8081_0000,
                0x200,
                &[(0x20, 8, 0x2222)],
            ));

            let loads = [(first, 0x8030_0000), (second, 0x8040_0000)];
            if reverse {
                for &(section, base) in loads.iter().rev() {
                    reg.set_section_loaded_at(section, base);
                }
                assert_eq!(reg.plan_static_mirror(0x2000, 0x20), Some(0x81_0000));
                assert_eq!(reg.plan_static_mirror(0x1000, 0x10), Some(0x80_0000));
            } else {
                for &(section, base) in &loads {
                    reg.set_section_loaded_at(section, base);
                }
                assert_eq!(reg.plan_static_mirror(0x1000, 0x10), Some(0x80_0000));
                assert_eq!(reg.plan_static_mirror(0x2000, 0x20), Some(0x81_0000));
                assert_eq!(reg.plan_static_mirror(0x1000, 0x10), Some(0x80_0000));
            }
            reg
        }

        assert_eq!(
            registry(false).evidence_snapshot(),
            registry(true).evidence_snapshot()
        );
    }

    #[test]
    fn evidence_snapshot_detects_section_and_function_metadata_mutations() {
        fn snapshot(
            rom_addr: u32,
            ram_addr: u32,
            size: u32,
            funcs: &[(u32, u32, usize)],
        ) -> SectionRegistryEvidenceSnapshot {
            let mut reg = SectionRegistry::new();
            reg.register_section(evidence_section(rom_addr, ram_addr, size, funcs));
            reg.evidence_snapshot()
        }

        let base = snapshot(0x1000, 0x8080_0000, 0x100, &[(0x10, 0x20, 1)]);
        assert_ne!(
            snapshot(0x1004, 0x8080_0000, 0x100, &[(0x10, 0x20, 1)]),
            base
        );
        assert_ne!(
            snapshot(0x1000, 0x8080_0010, 0x100, &[(0x10, 0x20, 1)]),
            base
        );
        assert_ne!(
            snapshot(0x1000, 0x8080_0000, 0x104, &[(0x10, 0x20, 1)]),
            base
        );
        assert_ne!(
            snapshot(0x1000, 0x8080_0000, 0x100, &[(0x14, 0x20, 1)]),
            base
        );
        assert_ne!(
            snapshot(0x1000, 0x8080_0000, 0x100, &[(0x10, 0x24, 1)]),
            base
        );

        let mut declared = SectionRegistry::new();
        declared.register_section(evidence_section(0x1000, 0x8080_0000, 0x100, &[]));
        declared.register_section(evidence_section(0x2000, 0x8081_0000, 0x100, &[]));
        let mut reversed = SectionRegistry::new();
        reversed.register_section(evidence_section(0x2000, 0x8081_0000, 0x100, &[]));
        reversed.register_section(evidence_section(0x1000, 0x8080_0000, 0x100, &[]));
        assert_ne!(declared.evidence_snapshot(), reversed.evidence_snapshot());
    }

    #[test]
    fn evidence_snapshot_detects_residency_and_runtime_load_mutations() {
        fn registry() -> (SectionRegistry, SectionIndex) {
            let mut reg = SectionRegistry::new();
            let idx =
                reg.register_section(evidence_section(0x1000, 0x8080_0000, 0x100, &[(0, 4, 1)]));
            (reg, idx)
        }

        let (unloaded, _) = registry();
        let (mut static_load, static_idx) = registry();
        static_load.set_section_loaded(static_idx);
        assert_ne!(
            unloaded.evidence_snapshot(),
            static_load.evidence_snapshot()
        );

        let (mut runtime_load, runtime_idx) = registry();
        runtime_load.set_section_loaded_at(runtime_idx, 0x8030_0000);
        assert_ne!(
            static_load.evidence_snapshot(),
            runtime_load.evidence_snapshot()
        );
    }

    #[test]
    fn evidence_snapshot_detects_static_storage_and_exact_cursor_mutations() {
        fn registry() -> SectionRegistry {
            let mut reg = SectionRegistry::new();
            reg.register_section(evidence_section(0x1000, 0x8080_0000, 0x100, &[]));
            reg.register_section(evidence_section(0x2000, 0x8081_0000, 0x100, &[]));
            reg
        }

        let mut active = registry();
        assert_eq!(active.plan_static_mirror(0x1000, 0x20), Some(0x80_0000));
        let active_snapshot = active.evidence_snapshot();
        assert_eq!(
            active_snapshot.static_mirror,
            Some(StaticMirrorEvidenceSnapshot {
                section: 0,
                next_rom: 0x1020,
                next_static_off: 0x80_0020,
            })
        );

        let mut stopped = registry();
        assert_eq!(stopped.plan_static_mirror(0x1000, 0x20), Some(0x80_0000));
        assert_eq!(stopped.plan_static_mirror(0x3000, 4), None);
        assert_eq!(
            active_snapshot.static_storage_ends,
            stopped.evidence_snapshot().static_storage_ends
        );
        assert_ne!(active_snapshot, stopped.evidence_snapshot());

        let mut extra_storage = registry();
        assert_eq!(
            extra_storage.plan_static_mirror(0x2000, 0x10),
            Some(0x81_0000)
        );
        assert_eq!(
            extra_storage.plan_static_mirror(0x1000, 0x20),
            Some(0x80_0000)
        );
        assert_eq!(
            active_snapshot.static_mirror,
            extra_storage.evidence_snapshot().static_mirror
        );
        assert_ne!(active_snapshot, extra_storage.evidence_snapshot());
    }
}
