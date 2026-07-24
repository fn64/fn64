use super::*;

// ---------------------------------------------------------------------
// recomp.h dispatch helpers: get_function / switch_error / do_break /
// pause_self. Real signatures per module doc.
// ---------------------------------------------------------------------

/// `pause_self(uint8_t *rdram)` -- ONE argument, no `ctx` (see module doc).
///
/// Parks the calling guest thread (`Yield::StopSelf`: `ThreadState::Stopped`
/// until an explicit `osStartThread`), matching the reference
/// N64ModernRuntime. It must NOT auto-resume: N64Recomp's C codegen emits
/// this call for an unconditional guest self-branch with NO loop back, so
/// returning here executes code the console can never reach. The WM2000
/// frontier bug this fixes (2026-07-21): the sound driver's file-id bounds
/// assert (`func_80003DD4` 0x80003DF0, `j self` hang) was fallen through,
/// the out-of-range id (0xC5BE) indexed past the file tables, and the
/// resulting garbage lookup submitted a PI DMA sized 0xFFFFFFFC (-4). See
/// `Yield::StopSelf`'s doc comment for the full mechanism.
#[no_mangle]
pub extern "C" fn pause_self(_rdram: *mut u8) {
    suspend_active_coroutine(Yield::StopSelf);
}

/// `switch_error(const char* func, uint32_t vram, uint32_t jtbl)` --
/// N64Recomp's codegen'd jump-table-miss trap (M1-WORKLIST.md #2, 214 NWXE
/// call sites). This is, by design, ALWAYS a loud trap: real generated code
/// only reaches this when a `jr`-based switch's computed target matched no
/// known case, which is either a genuine bug in the recompiled program or
/// (far more likely at this project's current stage) a section not yet
/// registered/loaded in `SectionRegistry` -- either way, per `AGENTS.md`,
/// this must never return normally with a guessed fallback.
///
/// # Safety
/// `func` must be a valid nul-terminated C string pointer, as every real
/// call site (`switch_error(__func__, vram, jtbl)`) provides via the
/// compiler's own `__func__`.
#[no_mangle]
pub unsafe extern "C" fn switch_error(func: *const std::os::raw::c_char, vram: u32, jtbl: u32) {
    let func_name = if func.is_null() {
        "<null>".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(func) }
            .to_string_lossy()
            .into_owned()
    };
    panic!(
        "switch_error: jump-table dispatch in {func_name} found no matching case for vram \
         {vram:#010x} (jump table at {jtbl:#010x}) -- this is N64Recomp's own generated \
         safety net firing, meaning either a genuine bug in the recompiled program or (more \
         likely at this project's stage) this address's target was never correctly resolved. \
         Per AGENTS.md, this is a loud trap by design, never a silently-guessed fallback."
    );
}

/// `do_break(uint32_t vram)` -- N64Recomp's codegen'd `break`/assert trap
/// (M1-WORKLIST.md #3, 253 NWXE call sites).
#[no_mangle]
pub extern "C" fn do_break(vram: u32) {
    panic!(
        "do_break: recompiled MIPS `break`/undefined-instruction trap fired at vram \
         {vram:#010x} -- this is N64Recomp's own generated safety net, not a silent no-op \
         per AGENTS.md."
    );
}

/// `get_function(int32_t vram) -> recomp_func_t*` -- the overlay/section
/// resolver every `LOOKUP_FUNC` call site depends on (M1-WORKLIST.md #1,
/// 85 NWXE call sites via `LOOKUP_FUNC`). Backed by `SectionRegistry`,
/// registered via `register_section`/`set_section_loaded` below (this
/// crate's own public API, called by `fn64-shell`/tests once at startup
/// from the game's `recomp_overlays.inl`-derived data -- `fn64-runtime`
/// itself has no `.inl`-parsing knowledge, per `docs/DESIGN.md` section 1's
/// crate split).
///
/// Resolution is deliberately not execution evidence. The returned pointer
/// may be compared, cached, or discarded. Official generated-C builds inject
/// [`fn64_c_recompiled_function_enter`] inside every generated body, which is
/// the first boundary that proves native control actually entered it.
#[no_mangle]
pub extern "C" fn get_function(vram: i32) -> *const () {
    with_host(|host| host.sections.resolve(vram as u32) as *const ())
}

/// Record entry into one prepared N64Recomp-generated native function.
///
/// `fn64-boot-harness` injects this call immediately inside every generated
/// `RECOMP_FUNC` body. A lookup alone never reaches this hook, while direct
/// generated C-to-C calls do, so this is execution evidence for the prepared
/// native archive rather than a log of resolution attempts.
#[no_mangle]
pub extern "C" fn fn64_c_recompiled_function_enter(function: RecompFunc) {
    let at = Cycles::new(sim_time());
    with_host(|host| {
        let pointer = function as usize;
        let destination = *host
            .native_destination_by_pointer
            .get(&pointer)
            .unwrap_or_else(|| {
                panic!(
                    "fn64_c_recompiled_function_enter: entered native callable {pointer:#x} was not registered in the generated section table"
                )
            });
        host.native_execution_destinations
            .push(NativeExecutionDestinationEvent { at, destination });
    });
}

/// Copy successfully entered native generated-C destinations in exact entry
/// order. The history is append-only until a new ROM/process lifetime begins.
pub fn copy_native_execution_destinations() -> Vec<NativeExecutionDestinationEvent> {
    with_host(|host| host.native_execution_destinations.clone())
}

/// Register a section's `FuncEntry` table with the overlay registry, in the
/// exact shape `ABI-SURFACE.md` section (d) documents (`rom_addr`/
/// `ram_addr`/`size` plus a `(offset, func)` list) -- called once per
/// generated `SectionTableEntry`, in the same order the generated
/// `.inl`'s `section_table[]` declares them, so the returned
/// `SectionIndex` matches `SectionTableEntry.index`.
///
/// # Safety
/// Every `func` pointer must be a valid `RecompFunc` for the lifetime of
/// the process (true for every `FuncEntry.func` in generated C, which are
/// all file-scope `RECOMP_FUNC` definitions with static storage duration).
pub unsafe fn register_section(
    rom_addr: u32,
    ram_addr: u32,
    size: u32,
    funcs: &[(u32, u32, RecompFunc)],
) -> fn64_runtime::SectionIndex {
    let entries = funcs
        .iter()
        .map(|&(offset, rom_size, func)| fn64_runtime::FuncEntry {
            func_ptr: func as usize,
            offset,
            rom_size,
        })
        .collect();
    with_host(|host| {
        let section_index = host.sections.register_section(Section {
            rom_addr,
            ram_addr,
            size,
            funcs: entries,
        });
        let stable_section_index = u32::try_from(section_index)
            .expect("generated section index exceeds native destination evidence wire");
        for &(offset, rom_size, function) in funcs {
            let link_vram = ram_addr.checked_add(offset).unwrap_or_else(|| {
                panic!(
                    "registered function offset {offset:#x} overflows section link base {ram_addr:#010x}"
                )
            });
            // A `rom_size == 0` FuncEntry is a HOST SHIM, not recompiled code:
            // it names one of fn64-abi's own `os*_recomp` override symbols (a
            // hand-written Rust body with no ROM origin), whereas a real
            // recompiled body always carries the byte length of the MIPS it was
            // lowered from (see `recomp_overlays.inl`: `sqrtf_recomp` .rom_size
            // = 0x10 vs. every `os*_recomp` override .rom_size = 0).
            //
            // Host shims are NEVER instrumented with
            // `fn64_c_recompiled_function_enter` -- that observer is injected
            // only into generated `RECOMP_FUNC` bodies
            // (fn64-boot-harness/build_support.rs) -- so a shim's pointer is
            // never looked up in `native_destination_by_pointer` at run time.
            // Registering it therefore yields zero observability, and doing so
            // is the sole source of a legitimate false collision: the optimizer
            // (identical-code folding) may collapse two distinct zero-body
            // shims to one code address, mapping one native pointer to two
            // guest destinations. That fold is correct optimizer behavior, so
            // we skip shims entirely and keep the strict 1:1 assertion for real
            // recompiled bodies, where a pointer collision IS a genuine
            // miscompile that would silently corrupt the execution-order log.
            if rom_size == 0 {
                continue;
            }
            let destination = NativeExecutionDestination {
                section_index: stable_section_index,
                function_offset: offset,
                link_vram,
            };
            match host.native_destination_by_pointer.entry(function as usize) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(destination);
                }
                std::collections::hash_map::Entry::Occupied(entry) => {
                    assert_eq!(
                        *entry.get(),
                        destination,
                        "one native callable pointer is assigned to multiple generated destinations"
                    );
                }
            }
        }
        section_index
    })
}

/// Register section geometry for a typed recompiled module. Function pointers
/// stay in that module's safe dispatcher; this registry owns only the
/// ROM/static/runtime-base mapping needed to canonicalize relocated callbacks.
pub fn register_recompiled_section(
    rom_addr: u32,
    ram_addr: u32,
    size: u32,
) -> fn64_runtime::SectionIndex {
    with_host(|host| {
        host.sections.register_section(Section {
            rom_addr,
            ram_addr,
            size,
            funcs: Vec::new(),
        })
    })
}

pub fn set_section_loaded(index: fn64_runtime::SectionIndex) {
    with_host(|host| host.sections.set_section_loaded(index));
}

pub fn set_section_unloaded(index: fn64_runtime::SectionIndex) {
    with_host(|host| host.sections.set_section_unloaded(index));
}

/// Honor a game-driven overlay DMA at the section registry: if `rom_addr`
/// is exactly some registered section's ROM start, mark it loaded at
/// `dest_vram` (the DMA's RDRAM destination as a KSEG0 vram). Returns the
/// section index if a load happened, else `None` (an ordinary data DMA).
/// Called from the PI/EPI DMA shims so overlays the game DMAs in become
/// resolvable at their true relocated base -- see
/// `SectionRegistry::load_section_at_rom_addr`.
pub fn note_dma_overlay_load(
    rom_addr: u32,
    dest_vram: u32,
    len: u32,
) -> Option<fn64_runtime::SectionIndex> {
    let (loaded, covered) = with_host(|host| {
        // Exact-match path: an overlay DMA'd whole to some (possibly relocated)
        // base -- OoT actor/gamestate overlays keyed on the exact section start.
        let exact = host.sections.load_section_at_rom_addr(rom_addr, dest_vram);
        // Coverage path: a chunk of a contiguous multi-section segment landing
        // at its static link VRAM (SM64's chunked engine-segment DMA). Marks
        // every section this chunk touches at its static base.
        let covered = host
            .sections
            .load_sections_covered_by_dma(rom_addr, dest_vram, len);
        (exact, covered)
    });
    if std::env::var("FN64_DEBUG_BOOT").is_ok() {
        eprintln!(
            "[DEBUG note_dma_overlay_load] rom={rom_addr:#010x} dest={dest_vram:#010x} \
             len={len:#x} -> exact={loaded:?} covered={covered:?}"
        );
    }
    // Prefer the exact-match index for callers that read it; fall back to the
    // first covered section so a chunked static-image load still reports one.
    loaded.or_else(|| covered.first().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    // These two markers must have DISTINCT addresses: the code under test keys
    // `native_destination_by_pointer` on the function pointer. Their bodies are
    // otherwise near-identical, so under `--release` the linker's
    // identical-code-folding merged them to one address — making both register
    // under one pointer and tripping the "one pointer, two destinations"
    // assert. A distinct `black_box` value per fn keeps them un-foldable.
    unsafe extern "C" fn observed_native_first(_rdram: *mut u8, _ctx: *mut RecompContext) {
        std::hint::black_box(0xF187u16);
        fn64_c_recompiled_function_enter(observed_native_first);
    }

    unsafe extern "C" fn observed_native_second(_rdram: *mut u8, _ctx: *mut RecompContext) {
        std::hint::black_box(0x5EC0u16);
        fn64_c_recompiled_function_enter(observed_native_second);
    }

    #[test]
    fn native_destinations_record_entry_not_lookup_and_preserve_direct_call_order() {
        with_host(|host| *host = HostState::default());
        load_rom(Vec::new());
        let section = unsafe {
            register_section(
                0x0010_0000,
                0x8000_4000,
                0x100,
                &[
                    (0x10, 4, observed_native_first),
                    (0x40, 4, observed_native_second),
                ],
            )
        };
        set_section_loaded(section);

        let resolved = get_function(0x8000_4010u32 as i32);
        assert!(copy_native_execution_destinations().is_empty());

        let mut context = RecompContext::zeroed();
        let first: RecompFunc = unsafe { std::mem::transmute(resolved) };
        unsafe { first(std::ptr::null_mut(), &mut context) };
        unsafe { observed_native_second(std::ptr::null_mut(), &mut context) };

        let section_index = u32::try_from(section).unwrap();
        assert_eq!(
            copy_native_execution_destinations(),
            vec![
                NativeExecutionDestinationEvent {
                    at: Cycles::new(0),
                    destination: NativeExecutionDestination {
                        section_index,
                        function_offset: 0x10,
                        link_vram: 0x8000_4010,
                    },
                },
                NativeExecutionDestinationEvent {
                    at: Cycles::new(0),
                    destination: NativeExecutionDestination {
                        section_index,
                        function_offset: 0x40,
                        link_vram: 0x8000_4040,
                    },
                },
            ]
        );

        load_rom(Vec::new());
        assert!(copy_native_execution_destinations().is_empty());
        with_host(|host| *host = HostState::default());
    }

    // A single host shim body used to model two DISTINCT zero-size shims that
    // the optimizer (identical-code folding) collapsed to one code address:
    // registering the SAME native pointer under two different guest
    // destinations is exactly what `register_section` observes post-fold.
    // fn64-abi's own `os*_recomp` overrides are never `RECOMP_FUNC` bodies, so
    // they carry no `fn64_c_recompiled_function_enter` and are never looked up
    // in `native_destination_by_pointer` -- their registration would only ever
    // trip the 1:1 assert.
    unsafe extern "C" fn folded_host_shim(_rdram: *mut u8, _ctx: *mut RecompContext) {}

    #[test]
    fn folded_zero_size_host_shims_register_without_panicking_and_are_not_mapped() {
        with_host(|host| *host = HostState::default());
        load_rom(Vec::new());

        // SM64 sections 66 (osSpTaskYielded_recomp) and 80 (osEepromProbe_recomp)
        // are distinct single-func rom_size==0 host shims; under --release the
        // optimizer folds them to one native pointer. Model that: two sections,
        // same pointer, both rom_size==0. This must not panic.
        let yielded = unsafe {
            register_section(0x0020_0000, 0x8032_2D70, 0x10, &[(0x0, 0, folded_host_shim)])
        };
        let eeprom = unsafe {
            register_section(0x0020_1000, 0x8032_4080, 0x10, &[(0x0, 0, folded_host_shim)])
        };
        set_section_loaded(yielded);
        set_section_loaded(eeprom);

        // Both guest VAs still resolve to the shim body through the vram-keyed
        // section registry (the execution-critical path is untouched)...
        assert_eq!(
            get_function(0x8032_2D70u32 as i32) as usize,
            folded_host_shim as *const () as usize
        );
        assert_eq!(
            get_function(0x8032_4080u32 as i32) as usize,
            folded_host_shim as *const () as usize
        );
        // ...but the folded shim pointer was deliberately NOT inserted into the
        // observability map, so the false 1:1 collision never arises.
        with_host(|host| {
            assert!(host.native_destination_by_pointer.is_empty());
        });

        with_host(|host| *host = HostState::default());
    }

    // A genuine pointer collision between two REAL recompiled bodies
    // (rom_size != 0) is a miscompile that would silently corrupt the entry
    // log; the strict 1:1 assertion must still fire for that case.
    unsafe extern "C" fn collided_recompiled_body(_rdram: *mut u8, _ctx: *mut RecompContext) {}

    #[test]
    fn genuine_collision_of_two_real_recompiled_bodies_still_asserts() {
        with_host(|host| *host = HostState::default());
        load_rom(Vec::new());

        let registration = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            // Same native pointer, two distinct real (rom_size != 0)
            // destinations -> a true miscompile the assert must catch.
            register_section(0x0030_0000, 0x8040_0000, 0x100, &[(0x0, 4, collided_recompiled_body)]);
            register_section(0x0031_0000, 0x8041_0000, 0x100, &[(0x0, 4, collided_recompiled_body)]);
        }));
        assert!(
            registration.is_err(),
            "two real recompiled bodies sharing one native pointer must still trip the 1:1 assert"
        );

        with_host(|host| *host = HostState::default());
    }

    #[test]
    fn get_function_miss_panics_naming_the_vram() {
        assert_subprocess_aborts("dispatch::tests::__get_function_miss_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __get_function_miss_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            get_function(0x1234_5678u32 as i32);
        }
    }

    #[test]
    fn pause_self_outside_any_coroutine_panics_loudly() {
        assert_subprocess_aborts("dispatch::tests::__pause_self_no_yielder_abort_subprocess_entry");
    }

    #[test]
    #[ignore] // only ever run directly, by the subprocess harness above
    fn __pause_self_no_yielder_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            pause_self(std::ptr::null_mut());
        }
    }

    #[test]
    fn switch_error_always_aborts() {
        assert_subprocess_aborts("dispatch::tests::__switch_error_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __switch_error_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            let func = std::ffi::CString::new("test_func").unwrap();
            unsafe { switch_error(func.as_ptr(), 0x8002_ABCC, 0x8004_B130) };
        }
    }

    #[test]
    fn do_break_always_aborts() {
        assert_subprocess_aborts("dispatch::tests::__do_break_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __do_break_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            do_break(0x8000_1234);
        }
    }

    /// Regression test for the fully-static-overlay NULL-function-pointer trap
    /// (2026-07-15): OoT's Player overlay (section 9) is a STATIC N64Recomp
    /// build (num_relocs=0, zero RELOC_HI16/LO16), so `Player_InitItemAction`
    /// reads `sItemActionInitFuncs[]` via a BAKED absolute link address
    /// (`lui 0x8085; lw 0x1EA8` -> 0x80851EA8, funcs_67.c:1815-1830). The game
    /// DMAs the overlay to an arena HEAP base and relocates the HEAP copy, but
    /// the recompiled code dereferences the STATIC link VRAM -- which fn64 left
    /// unwritten (reads 0) -> `LOOKUP_FUNC(0)` trap. The fix mirrors the
    /// overlay's raw ROM image to its static link VRAM (holding un-relocated
    /// static function pointers) so the baked read resolves via the section's
    /// static base.
    ///
    /// This drives the real `osEPiStartDma_recomp` path: register a section at
    /// a static VRAM, DMA its ROM image to a HEAP vram (as the arena would),
    /// then assert the guest's own MEM_W read of the data-table entry at the
    /// STATIC link VRAM returns the static function pointer, and that
    /// `get_function` resolves it to the registered func. A DISTINCT func_ptr
    /// (not 0) makes a mirror miss / wrong-offset fail loudly. Without the
    /// mirror, the static-VRAM word is 0 and `get_function(0)` traps -- the
    /// exact bug this guards. (Verified fail-against-bug: reverting the mirror
    /// write makes the MEM_W read 0 and the resolve panic.)
    #[test]
    fn static_overlay_data_table_read_resolves_via_static_vram_mirror() {
        unsafe extern "C" fn player_init_default_ia(_r: *mut u8, _c: *mut RecompContext) {}
        let func_ptr: RecompFunc = player_init_default_ia;

        // Model section 9: rom 0x00BCDB70, static link VRAM 0x808301C0. The
        // data-table entry we test points at section offset 0x15E4 (the real
        // Player_InitDefaultIA offset). Use a compact ROM: the section image
        // is [text .. data], with the table living at file offset 0x40 and the
        // pointed-at func at static offset 0x15E4.
        const SEC_ROM: u32 = 0x00BC_DB70;
        const SEC_RAM: u32 = 0x8083_01C0;
        const FUNC_OFF: u32 = 0x15E4; // real Player_InitDefaultIA offset
        const TABLE_FILE_OFF: usize = 0x40; // where the ptr table sits in ROM
        let static_ptr: u32 = SEC_RAM + FUNC_OFF; // 0x808317A4 -- the baked static ptr
                                                  // The overlay file must be word-aligned and long enough to DMA in one
                                                  // aligned chunk covering the table.
        let file_len: u32 = 0x80;

        // Register the section with a FuncEntry at the pointed-at offset, and
        // register a SECOND, unrelated section so a wrong-section resolve is
        // caught. Only mark section 9 loaded (at its heap base, as the arena
        // DMA would) so `resolve`'s static-base path is what's exercised.
        let sec9 =
            unsafe { register_section(SEC_ROM, SEC_RAM, 0x2_1110, &[(FUNC_OFF, 4, func_ptr)]) };

        // Build a ROM whose section image holds the static pointer (big-endian
        // cart order) at the table offset.
        let mut rom = vec![0u8; (SEC_ROM as usize) + (file_len as usize) + 0x10];
        let tbl_rom = SEC_ROM as usize + TABLE_FILE_OFF;
        rom[tbl_rom..tbl_rom + 4].copy_from_slice(&static_ptr.to_be_bytes());
        load_rom(rom);

        // DMA the overlay's ROM image to a HEAP base (as the arena allocates),
        // via the real osEPiStartDma_recomp shim. Heap base picked to differ
        // from the static VRAM so a heap-vs-static confusion is caught.
        let heap_vram: u32 = 0x8038_8b60;
        let mut rdram = vec![0u8; fn64_runtime::RDRAM_MMIO_WINDOW_END as usize];
        set_cart_rom_handle_vram(0x8000_1000);
        let mut cart = ctx_zeroed();
        unsafe { osCartRomInit_recomp(rdram.as_mut_ptr(), &mut cart) };
        let mb_off = 0x2000usize;
        let mb_vram: u64 = 0x8000_2000;
        rdram[mb_off + 0x4..mb_off + 0x8].copy_from_slice(&0u32.to_ne_bytes()); // retQueue
        rdram[mb_off + 0x8..mb_off + 0xC].copy_from_slice(&heap_vram.to_ne_bytes()); // dramAddr
        rdram[mb_off + 0xC..mb_off + 0x10].copy_from_slice(&SEC_ROM.to_ne_bytes()); // devAddr
        rdram[mb_off + 0x10..mb_off + 0x14].copy_from_slice(&file_len.to_ne_bytes()); // size

        let mut ctx = ctx_zeroed();
        ctx.r4 = cart.r2;
        ctx.r5 = mb_vram;
        ctx.r6 = 0; // OS_READ / ToRdram
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        advance_virtual_time(1);

        // The game's baked read: MEM_W at the STATIC link VRAM of the table.
        // Static VRAM 0x808301C0+TABLE_FILE_OFF -> rdram offset (mask KSEG0).
        let table_static_vram = SEC_RAM + TABLE_FILE_OFF as u32;
        let static_off = (table_static_vram & 0x1FFF_FFFF) as usize;
        let read_ptr = u32::from_ne_bytes(rdram[static_off..static_off + 4].try_into().unwrap());
        assert_eq!(
            read_ptr, static_ptr,
            "static-link-VRAM mirror must place the un-relocated static function pointer \
             ({static_ptr:#010x}) where the baked `lw` reads it; got {read_ptr:#010x} (0 == \
             mirror never ran -> the LOOKUP_FUNC(0) trap)"
        );
        // And it resolves through the section's static base to the real func.
        let resolved = get_function(read_ptr as i32);
        assert_eq!(
            resolved as usize, func_ptr as usize,
            "the mirrored static pointer must resolve to the registered FuncEntry"
        );

        set_section_unloaded(sec9);
    }
}
