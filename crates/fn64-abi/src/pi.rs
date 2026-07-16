use super::*;

/// Install the real ROM bytes the PI/EPI DMA shims read from. Must be
/// called once before any `osEPiStartDma_recomp`/`osCartRomInit_recomp`
/// call, per `README.md`'s "no game content ships in this repo" rule --
/// `fn64-shell` supplies the user's own loaded ROM file's bytes here.
pub fn load_rom(bytes: Vec<u8>) {
    with_host(|host| host.pi_dma = Some(PiDma::new(InMemoryRom::new(bytes))));
}

/// Register the guest BSS address of libultra's cartridge `OSPiHandle`.
///
/// `osCartRomInit` returns an `OSPiHandle*` that ordinary recompiled game code
/// may dereference before passing it back to an EPI shim. The address therefore
/// cannot be an opaque host token: it must be the aligned, guest-visible BSS
/// object from this particular ROM's link map.
pub fn set_cart_rom_handle_vram(vram: u32) {
    assert!(
        (0x8000_0000..0xC000_0000).contains(&vram),
        "cart OSPiHandle must be a KSEG0/KSEG1 guest address, got {vram:#010x}"
    );
    assert!(
        vram.is_multiple_of(4),
        "cart OSPiHandle must be word-aligned, got {vram:#010x}"
    );
    with_host(|host| host.cart_rom_handle_vram = Some(vram));
}

/// Register the game's save-backing store (SRAM/EEPROM/Flash) the domain-2
/// PI-DMA path routes to -- `fn64-shell`/the harness supplies an
/// `InMemorySaveStorage`/`FileSaveStorage` sized for the game's save device
/// (OoT: `SaveType::SramBanked`, 32 KiB). Must be called after `load_rom`
/// (the `PiDma` engine must exist) and before any domain-2 (SRAM) DMA. A
/// domain-2 DMA with no save registered is a loud trap, not a silent ROM
/// read past its end (see `PiDma::set_save`).
pub fn set_save(save: Box<dyn fn64_runtime::SaveStorage>) {
    with_pi_dma("set_save", |dma| dma.set_save(save));
}

fn with_pi_dma<R>(shim: &str, f: impl FnOnce(&mut PiDma<InMemoryRom>) -> R) -> R {
    with_host(|host| {
        let dma = host.pi_dma.as_mut().unwrap_or_else(|| {
            panic!(
                "{shim}: no ROM installed -- call fn64_abi::load_rom(bytes) before any PI/EPI \
                 DMA shim runs (see that function's doc comment; this crate never ships game \
                 content, so there is no default ROM to fall back to)"
            )
        });
        f(dma)
    })
}

// ---------------------------------------------------------------------
// PI/ROM seam: osCartRomInit / osEPiStartDma / osVirtualToPhysical /
// osCreatePiManager / __osSiRawStartDma / osSetIntMask / osInitialize /
// osAiSetFrequency / osSpTaskYielded.
// ---------------------------------------------------------------------

/// `osCartRomInit(void) -> OSPiHandle*` -- no arguments (verified: every
/// real call site is `osCartRomInit_recomp(rdram, ctx)` with no register
/// setup beforehand). The PI engine remains host-owned, but the returned
/// pointer is not optional or opaque: guest code dereferences the handle's
/// public `transferInfo` fields before calling the host DMA shim. Return the
/// game-owned BSS address registered by [`set_cart_rom_handle_vram`].
///
/// OoT exposed the old no-op at `AudioLoad_Dma` ROM PC `0x800B824C`: its
/// aligned `sw $t1, 0x14($a0)` consumed `gAudioCtx.cartHandle`, which
/// `AudioLoad_Init` had populated from this return value. Leaving `$v0`
/// untouched propagated a stale, unaligned address into that store. The C
/// lane's raw memory macro tolerated the host-unaligned write; typed Rust's
/// alignment trap correctly refused it.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osCartRomInit_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    with_pi_dma("osCartRomInit_recomp", |_dma| {});
    let handle_vram = with_host(|host| {
        host.cart_rom_handle_vram.unwrap_or_else(|| {
            panic!(
                "osCartRomInit_recomp: no guest OSPiHandle address registered -- call \
                 fn64_abi::set_cart_rom_handle_vram with the ROM's aligned __CartRomHandle \
                 BSS address before boot"
            )
        })
    });
    let ctx = unsafe { &mut *ctx };
    ctx.r2 = (handle_vram as i32 as i64) as u64;
}

/// `osEPiStartDma(OSPiHandle *handle, OSIoMesg *mb, s32 direction)` --
/// `a0`=handle (`ctx->r4`, unused per `osCartRomInit_recomp`'s doc comment),
/// `a1`=mb (`ctx->r5`, an `OSIoMesg*`), `a2`=direction (`ctx->r6`,
/// `OS_READ`=0/`OS_WRITE`=1 per the public manual).
///
/// The `OSIoMesg` field offsets are byte-verified against the OoT decomp
/// header (`oot-decomp/include/ultra64/pi.h`) AND cross-checked against
/// DmaMgr's own stack-struct build in OOTU `funcs_0.c`
/// `DmaMgr_DmaRomToRam` (asm 0x800008F0-0x80000900): `OSIoMesgHdr` is only
/// 0x08 bytes (`type` +0x0, `pri` +0x2, `status` +0x3, `retQueue` +0x4),
/// so `dramAddr` is at +0x8, `devAddr` at +0xC, `size` at +0x10. A prior
/// wave wrongly assumed a 0xC (3-word) header and read every body field
/// +0x4 too high (size fell on the unwritten +0x14, reading 0) -- the OoT
/// `DmaMgr_Init` dmadata-DMA hang. See the inline comment at the field
/// reads below for the full store-to-field mapping. The DMA completion posts
/// through `Executor::inject_event(DirectPost)` -- the same "ONE explicit
/// host-side injection point" every other completion source uses
/// (`docs/DESIGN.md` section 2).
///
/// ## Correction (2026-07-14): must set `ctx.r2` (the `$v0` return value)
///
/// A prior wave never wrote a return value at all, leaving `ctx.r2` at
/// whatever stale value the caller's own earlier computation left there.
/// Real `osEPiStartDma` returns `s32`: 0 on successful enqueue, -1 if
/// `!__osPiDevMgr.active` (byte-identical shape confirmed against WCW
/// Revenge's `func_800219B0`,
/// `aki-recomp/refs/WCWnWoRevengeRecomp/disasm/libultra.md` ~line 213).
/// `examples/wm2000-boot`'s real boot run surfaced the consequence: the
/// chunked-DMA loop in NWXE's `func_80000660`
/// (`aki-recomp/games/NWXE/RecompiledFuncs/funcs_0.c`, asm
/// 0x800006E4-0x800006FC) re-issues `osEPiStartDma` while `$v0 != 0` and
/// only falls through to a blocking `osRecvMesg` once `$v0` reads exactly
/// 0 -- with `ctx.r2` never written, that test read garbage left over from
/// an earlier instruction (observed non-zero), so the loop re-issued the
/// same DMA chunk forever: a real, tens-of-seconds unbounded recompiled loop,
/// not a missing host model. This shim performs every DMA synchronously
/// and has no failure path today (`with_pi_dma` panics rather than
/// returning -1 when no ROM is installed, and `FromRdram` is an explicit
/// `unimplemented!()`), so every path that reaches the end of this
/// function represents success -- `ctx.r2 = 0` unconditionally there. A
/// real `-1` return only matters if/when this shim grows genuine
/// asynchronous PI-bus contention modeling, out of scope this wave.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osEPiStartDma_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mb_addr = RdramAddr::from_gpr(ctx.r5);
    let direction = if ctx.r6 == 0 {
        DmaDirection::ToRdram
    } else {
        DmaDirection::FromRdram
    };

    // OSIoMesg layout, byte-verified against the OoT decomp header
    // `oot-decomp/include/ultra64/pi.h`: `OSIoMesgHdr` is 0x08 bytes
    // (`u16 type` +0x0, `u8 pri` +0x2, `u8 status` +0x3, `OSMesgQueue*
    // retQueue` +0x4), NOT the 3-word (0xC) header a prior wave assumed.
    // The body follows immediately: `dramAddr` +0x8, `devAddr` +0xC,
    // `size` +0x10, `piHandle` +0x14. This exactly matches DmaMgr's own
    // struct build in OOTU `funcs_0.c` DmaMgr_DmaRomToRam (mb = $sp+0x70):
    // `sb $zero,0x72` (pri/status, +0x2), `sw $s6,0x74` (retQueue, +0x4),
    // `sw $s4,0x78` (dramAddr = a1/RAM dest, +0x8, asm 0x800008FC),
    // `sw $s2,0x7C` (devAddr = a0/romStart, +0xC, asm 0x800008F8),
    // `sw $s0,0x80` (size = chunk, +0x10, asm 0x80000900). The prior +0x4-
    // shifted offsets read dramAddr as retMesg, devAddr as dramAddr, size
    // as devAddr, and size from unwritten +0x14 (=0) -- the OoT DmaMgr_Init
    // hang: the dmadata DMA delivered len=0 and MEM_W(dest+4)!=0x1060 ->
    // Fault_AddHungupAndCrash (assert 0x345). There is no `retMesg` field.
    //
    // Correction (this wave): a prior wave called `read_stack_word` (which
    // itself calls `RdramAddr::from_gpr`, subtracting the KSEG0 base) with
    // `mb_addr.offset()` -- an ALREADY-rdram-relative offset (KSEG0 already
    // subtracted once, on line computing `mb_addr` above). Subtracting
    // KSEG0 a SECOND time produced a wildly wrong address, first caught by
    // `examples/wm2000-boot`'s actual boot run (a real EXC_BAD_ACCESS deep
    // in this function once boot finally reached its first real PI DMA,
    // thread 6's `func_800222D8` -> ... -> `osEPiStartDma_recomp` call
    // chain). Fixed via `read_offset_word` (below), a sibling helper that
    // takes an ALREADY-resolved rdram offset and does no further KSEG0
    // translation -- the two helpers now have distinct names specifically
    // so this class of double-translation mistake doesn't recur silently at
    // a future call site (per `AGENTS.md`'s "mechanism over patch": fixing
    // just this one call site without a differently-named sibling helper
    // would leave the same trap for the next `RdramAddr`-holding caller).
    let ret_queue = read_offset_word(rdram, mb_addr.offset(), 0x4);
    // No `retMesg` field exists (OSIoMesgHdr ends at retQueue); DmaMgr's
    // osRecvMesg waits on retQueue with a NULL msg-out pointer, so post a 0.
    let ret_mesg = 0u32;
    // dramAddr is a raw vram POINTER the game computed the normal way (e.g.
    // `&someBuffer`), same as any other vram value -- it needs the SAME
    // KSEG0 translation `RdramAddr::from_gpr` performs, not
    // `RdramAddr::from_offset` (which assumes the value is ALREADY an
    // rdram-relative offset with no translation needed). Using
    // `from_offset` here was a real bug (this field's value is a raw vram
    // address like any other, not a pre-resolved offset) -- caught by this
    // wave's own regression test after the sibling double-translation bug
    // (see the correction note above `read_offset_word`'s introduction).
    let dram_addr = RdramAddr::from_gpr(read_offset_word(rdram, mb_addr.offset(), 0x8) as u64);
    let dev_addr = read_offset_word(rdram, mb_addr.offset(), 0xC);
    let len = read_offset_word(rdram, mb_addr.offset(), 0x10);

    let completion = {
        let mut rt_rdram = fn64_runtime::Rdram::new(0);
        // Safety: fn64-abi does not own a fn64_runtime::Rdram wrapper (the
        // raw `rdram` pointer IS the shared buffer, per docs/DESIGN.md
        // section 3) -- construct a zero-length placeholder and instead
        // perform the copy directly against the raw pointer below, mirroring
        // osRecvMesg_recomp's existing pattern of not creating a second,
        // competing Rdram instance over borrowed memory.
        let _ = &mut rt_rdram;
        // Domain-2 (SRAM/save) DMAs route to the save store, not the ROM
        // image. `devAddr >= SRAM_DOMAIN2_BASE` (0x08000000, PI_DOM2_ADDR2,
        // OoT rcp.h:714) is a save access -- OoT's SsSram_ReadWrite passes
        // physical 0x08000000+offset as devAddr (z_sram.c:672 /
        // funcs_34.c:10632). A ROM-domain devAddr is a small ROM offset.
        let is_sram = fn64_runtime::rom::is_sram_dev_addr(dev_addr);
        with_pi_dma("osEPiStartDma_recomp", |dma| match direction {
            // device -> RDRAM (ROM read OR SRAM/save read). Both deliver a
            // FLAT big-endian/save-order byte buffer that must be
            // word-swizzled into rdram's native-word storage, because the
            // guest reads the destination via MEM_BU (`^3` XOR, recomp.h).
            // Same swizzle for ROM and SRAM; only the SOURCE differs.
            DmaDirection::ToRdram => {
                let mut buf = vec![0u8; len as usize];
                if is_sram {
                    dma.sram_read_into(dev_addr - fn64_runtime::rom::SRAM_DOMAIN2_BASE, &mut buf);
                } else {
                    dma.read_rom_bytes(dev_addr, &mut buf);
                }
                // Per-BYTE lane swizzle into native-word rdram, matching
                // `Rdram::dma_write_bytes` (`dst[(base+k)^3]=src[k]`). A flat
                // copy hung OoT's DmaMgr_Init (MEM_W(dest+4)==0x1060 saw
                // 0x60100000) and would corrupt every SRAM byte the save loader
                // MEM_BU's back. This shim copies through the raw rdram pointer
                // (no Rdram wrapper), so swizzle byte-by-byte here. Per-byte,
                // NOT per-word, so it stays correct for the sub-word / unaligned
                // transfers OoT's DmaMgr_DmaRomToRam issues (e.g. len=0x86).
                let base = dram_addr.offset() as usize;
                for (k, &b) in buf.iter().enumerate() {
                    unsafe {
                        *rdram.add((base + k) ^ 3) = b;
                    }
                }
                fn64_runtime::DmaCompletion {
                    direction,
                    dram_addr,
                    dev_addr,
                    len,
                }
            }
            // RDRAM -> device. Only the domain-2 (SRAM/save) case is real: the
            // guest's rdram source holds native-word-swizzled bytes, so
            // un-swizzle back to flat save order (inverse of the ToRdram
            // swizzle) before writing the save chip. A ROM-domain write is
            // still nonsensical (ROM is read-only) -> loud trap.
            DmaDirection::FromRdram => {
                if !is_sram {
                    unimplemented!(
                        "osEPiStartDma_recomp: OS_WRITE to the cartridge-ROM domain (devAddr \
                         {dev_addr:#x} < SRAM_DOMAIN2_BASE) -- ROM is read-only. A save write \
                         uses devAddr >= 0x08000000 and routes to the save store."
                    );
                }
                assert!(
                    dram_addr.offset().is_multiple_of(4) && (len as usize).is_multiple_of(4),
                    "PI DMA must be word-aligned (dram={:#x} len={:#x})",
                    dram_addr.offset(),
                    len
                );
                let mut buf = vec![0u8; len as usize];
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        rdram.add(dram_addr.offset() as usize),
                        buf.as_mut_ptr(),
                        buf.len(),
                    );
                }
                for word in buf.chunks_exact_mut(4) {
                    word.swap(0, 3);
                    word.swap(1, 2);
                }
                dma.sram_write_from(dev_addr - fn64_runtime::rom::SRAM_DOMAIN2_BASE, &buf);
                fn64_runtime::DmaCompletion {
                    direction,
                    dram_addr,
                    dev_addr,
                    len,
                }
            }
        })
    };

    if ret_queue != 0 {
        // retQueue (OSMesgHdr's OSMesgQueue*) is likewise a raw vram
        // pointer, same correction as dramAddr above -- from_gpr, not
        // from_offset.
        with_executor(|exec| {
            exec.inject_event(ExternalEvent::DirectPost {
                queue_addr: RdramAddr::from_gpr(ret_queue as u64),
                msg: ret_mesg,
            })
        });
    }
    let _ = completion;

    // Overlay-load hook: if this DMA's ROM source is exactly a registered
    // code section's start, the game just DMA'd that overlay in. Mark it
    // loaded at the DMA's RDRAM destination vram so a later LOOKUP_FUNC for
    // the game's relocated function pointers (Overlay_Relocate rewrites them
    // to `dest + offset`) resolves. Data DMAs (dmadata, objects) don't match
    // any section start and are a no-op here. Done AFTER the with_pi_dma /
    // with_host borrow closes to avoid re-entrant host access.
    // `dev_addr` is the ROM offset; `dram_addr.offset() | KSEG0` is the vram.
    let dest_vram = dram_addr.offset() | 0x8000_0000;
    note_dma_overlay_load(dev_addr, dest_vram);

    // Static-link-VRAM mirror: this is a fully-static N64Recomp build (every
    // section num_relocs=0, zero RELOC_HI16/LO16 in the generated C), so an
    // overlay's recompiled code reads its own DATA via BAKED absolute link
    // addresses (e.g. Player_InitItemAction's `lui 0x8085; lw 0x1EA8` reads
    // sItemActionInitFuncs[] at static 0x80851EA8), NOT the heap address the
    // game DMA'd the overlay to. Mirror the overlay's raw ROM image to its
    // static link VRAM so those baked reads find the (un-relocated) static
    // function pointers, which `resolve` maps through the section's static
    // base. Only for device->RDRAM ROM reads; SRAM/save DMAs never match a
    // code section. See SectionRegistry::plan_static_mirror.
    if matches!(direction, DmaDirection::ToRdram) && !fn64_runtime::rom::is_sram_dev_addr(dev_addr)
    {
        if let Some(static_off) = with_host(|host| host.sections.plan_static_mirror(dev_addr, len))
        {
            let mut buf = vec![0u8; len as usize];
            with_pi_dma("osEPiStartDma_recomp", |dma| {
                dma.read_rom_bytes(dev_addr, &mut buf)
            });
            // Same word-swizzle the primary destination write applies, so the
            // mirror is native-word storage the guest reads back via MEM_*.
            for word in buf.chunks_exact_mut(4) {
                word.swap(0, 3);
                word.swap(1, 2);
            }
            unsafe {
                std::ptr::copy_nonoverlapping(
                    buf.as_ptr(),
                    rdram.add(static_off as usize),
                    buf.len(),
                );
            }
        }
    }

    // Every path reaching here completed the DMA synchronously and
    // successfully -- see the doc comment's "Correction (2026-07-14)" for
    // why this must be written at all (a stale, unwritten $v0 caused a
    // real infinite retry loop in NWXE's chunked-DMA caller).
    ctx.r2 = 0;
}

/// `osVirtualToPhysical(void* vaddr) -> u32` -- KSEG0/1 virtual-to-physical
/// translation (M1-WORKLIST.md #15, highest call count in the whole
/// undefined set at 104x). Per the public libultra manual: for KSEG0/KSEG1
/// addresses (the only kind generated code passes -- MIPS o32 KSEG0 base
/// `0x80000000`/KSEG1 base `0xA0000000`), physical address is simply the
/// virtual address with the top 3 bits masked off (`vaddr & 0x1FFFFFFF`) --
/// documented, standard MIPS32 segment-translation arithmetic, not a
/// runtime-specific behavior. Returns the result in `ctx->r2` (`$v0`, the
/// o32 single-word return-value register).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osVirtualToPhysical_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let vaddr = ctx.r4 as u32;
    ctx.r2 = (vaddr & 0x1FFF_FFFF) as u64;
}

/// `osCreatePiManager(OSPri pri, OSMesgQueue *cmdQ, OSMesg *cmdBuf, s32
/// cmdMsgCnt)` -- spins up the PI-manager thread. Per `docs/DESIGN.md`
/// section 2's stackful-coroutine model, "the PI manager" is not a second
/// host thread in this design (there is exactly one executor thread) --
/// its role (serializing `osEPiStartDma` requests onto the single PI bus,
/// posting completions) is already what `osEPiStartDma_recomp` above does
/// directly and synchronously (module doc's "async-looking API" note in
/// `rom.rs`). This shim's real, tested effect is therefore just
/// registering `cmdQ` as a genuine `MesgQueue` (so a real ROM's own
/// `osSendMesg`/`osRecvMesg` calls against it, if any, behave correctly),
/// matching the one piece of `osCreatePiManager`'s documented contract this
/// milestone's evidence (rung 9) actually needs: a real, non-garbage
/// message queue existing at `cmdQ`'s address.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osCreatePiManager_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let cmd_q = RdramAddr::from_gpr(ctx.r5);
    let cmd_msg_cnt = ctx.r7 as usize;
    with_executor(|exec| exec.create_mesg_queue(cmd_q, cmd_msg_cnt.max(1)));
}

/// `__osPiGetAccess(void)` -- no arguments (verified: real call site
/// `funcs_0.c` asm 0x80001608, a bare `jal` with no register setup
/// immediately before it, same no-arg shape `osCartRomInit_recomp`'s doc
/// comment already established for this corpus's PI-bus bring-up
/// sequence). Real hardware effect: acquires the PI-bus mutex so this
/// thread has exclusive access for a following DMA/IO sequence. Per
/// `docs/DESIGN.md`'s single-executor-thread model there is no real
/// concurrent PI-bus contention to arbitrate (see `osSetIntMask_recomp`'s
/// doc comment for the identical reasoning already applied to the
/// interrupt-mask shim) -- a safe no-op beyond existing as a callable
/// symbol.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osPiGetAccess_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `__osPiRelAccess(void)` -- no arguments (verified: both real call sites
/// in `funcs_0.c`, asm 0x80001628 and 0x800017B8, are bare `jal`s with no
/// register setup beforehand). Releases the mutex `__osPiGetAccess_recomp`
/// acquires; same no-op reasoning.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osPiRelAccess_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `osEPiReadIo(OSPiHandle *handle, u32 devAddr, void *dramAddr) -> s32` --
/// `a0`=handle (`ctx->r4`, unused, same as `osEPiStartDma_recomp`'s
/// `osCartRomInit_recomp`-established handle stance), `a1`=devAddr
/// (`ctx->r5`), `a2`=dramAddr (`ctx->r6`) -- verified against the real call
/// site (`funcs_0.c:2611`: `ctx->r4=MEM_W(...)` a handle-shaped global,
/// `ctx->r5=0x3C` a devAddr, `ctx->r6=sp+0x24` a stack dramAddr). Public
/// libultra manual: a SYNCHRONOUS single 4-byte cartridge-domain read (no
/// `OSIoMesg`/queue involved, unlike `osEPiStartDma`'s async multi-byte
/// transfer) -- reads one word directly from ROM at `devAddr` into
/// `*dramAddr`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osEPiReadIo_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let dev_addr = ctx.r5 as u32;
    let dram_addr = RdramAddr::from_gpr(ctx.r6).offset() as usize;
    with_pi_dma("osEPiReadIo_recomp", |dma| {
        let mut buf = [0u8; 4];
        dma.read_rom_bytes(dev_addr, &mut buf);
        // Same word-swizzle as PiDma::start_dma / Rdram::write_bytes: rdram is
        // native-endian-WORD storage, so a big-endian cartridge word must be
        // stored byte-reversed or a later MEM_W/MEM_BU reads it swapped. A flat
        // copy here is exactly the bug that hung OoT's Locale_Init region check.
        let swz = [buf[3], buf[2], buf[1], buf[0]];
        unsafe {
            std::ptr::copy_nonoverlapping(swz.as_ptr(), rdram.add(dram_addr), 4);
        }
    });
    ctx.r2 = 0;
}

/// `osEPiWriteIo(OSPiHandle *handle, u32 devAddr, u32 data) -> s32` --
/// `a0`=handle (unused), `a1`=devAddr (`ctx->r5`), `a2`=data (`ctx->r6`).
/// Public libultra manual's synchronous single-word cartridge-domain
/// WRITE counterpart to `osEPiReadIo_recomp`. `PiDma`/`InMemoryRom` (this
/// crate's ROM backing) has no write-to-cart-domain support (`rom.rs`'s
/// `PiDma` doc: ROM is read-only host state) -- consistent with
/// `osEPiStartDma_recomp`'s existing `DmaDirection::FromRdram`
/// `unimplemented!()` stance for the same underlying gap, this is a loud
/// trap rather than a silent no-op, since a real cartridge write silently
/// discarded would be a correctness lie a differential trace could not
/// catch.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osEPiWriteIo_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "osEPiWriteIo_recomp: cartridge-domain writes have no backing store in this milestone \
         (InMemoryRom is read-only, matching osEPiStartDma_recomp's existing OS_WRITE gap) -- \
         real call site is games/OOTU/RecompiledFuncs/funcs_0.c, needs a real write-back model \
         before this can return anything but a loud trap."
    );
}

/// `osLeoDiskInit(void) -> s32` -- 64DD (Disk Drive) subsystem init. OoT
/// (OOTU) is a cartridge-only retail title; `leomain`/`LeoCJCreateLeoManager`
/// /`LeoCACreateLeoManager` (this symbol's only callers, newly reachable
/// after the 2026-07-14 stub-set fix) are 64DD-family debug/dev-kit code
/// paths dead on real retail hardware and never exercised by this crate's
/// PI/cartridge-only `InMemoryRom` model (`rom.rs`) -- no 64DD drive state
/// exists to initialize. Loud trap rather than a fabricated "drive present"
/// success/failure code.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osLeoDiskInit_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "osLeoDiskInit_recomp: no 64DD (Disk Drive) subsystem exists in this crate -- \
         OOTU is cartridge-only retail; reachable only from leomain/LeoC{{J,A}}CreateLeoManager \
         (games/OOTU/RecompiledFuncs), 64DD-family dev-kit code paths unstubbed by the \
         2026-07-14 gen_stubs.py false-positive-stub fix. A fabricated init result would be \
         an unearned guess about hardware this crate never models."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    #[test]
    fn os_virtual_to_physical_masks_kseg0() {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_1234;
        unsafe { osVirtualToPhysical_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0x0000_1234);
    }

    #[test]
    fn os_virtual_to_physical_masks_kseg1() {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0xA000_5678;
        unsafe { osVirtualToPhysical_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0x0000_5678);
    }

    /// Regression for OoT rs boot's `AudioLoad_Dma` alignment trap.
    /// `AudioLoad_Init` stores `osCartRomInit()`'s `$v0` into
    /// `gAudioCtx.cartHandle`; ROM PC 0x800B824C later executes the ordinary
    /// aligned `sw $t1, 0x14($a0)` through that pointer. The old shim left
    /// `$v0` untouched. Seed the exact stale value observed at the failing
    /// boot so that implementation returns `0x80125636` and fails this test,
    /// while the fixed shim returns the configured aligned guest handle.
    #[test]
    fn os_cart_rom_init_replaces_stale_unaligned_v0_with_guest_handle() {
        load_rom(vec![0u8; 0x100]);
        set_cart_rom_handle_vram(0x8000_9EA0);

        let mut ctx = ctx_zeroed();
        ctx.r2 = 0xFFFF_FFFF_8012_5636;
        unsafe { osCartRomInit_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };

        assert_eq!(ctx.r2, 0xFFFF_FFFF_8000_9EA0);
        assert_eq!(ctx.r2 & 3, 0, "returned OSPiHandle must be word-aligned");
    }

    /// Regression test for the real double-KSEG0-translation bug
    /// `examples/wm2000-boot`'s boot run surfaced (a genuine
    /// EXC_BAD_ACCESS deep in `osEPiStartDma_recomp`'s field reads, once
    /// boot finally reached its first real PI DMA on thread 6): `mb_addr`
    /// is placed at a REALISTIC nonzero vram address (not offset 0, which
    /// would hide the bug -- 0 minus 0 is still 0), and the OSIoMesg
    /// fields are placed at their real rdram offsets relative to that vram
    /// address, not relative to 0.
    /// Builds an OSIoMesg exactly as OOTU `DmaMgr_DmaRomToRam` does
    /// (`funcs_0.c` asm 0x800008F0-0x80000900): 0x08-byte `OSIoMesgHdr`
    /// (retQueue at +0x4), then `dramAddr` +0x8, `devAddr` +0xC, `size`
    /// +0x10. The prior version of this test placed fields +0x4 too high to
    /// match the buggy 0xC-header shim, so it passed green against the bug --
    /// the exact "weak green check" trap. A NON-UNIFORM multi-word ROM
    /// payload and a NON-ZERO multi-word `size` make a wrong-offset read
    /// (which would pick up size=0, or the wrong devAddr) fail loudly.
    #[test]
    fn os_epi_start_dma_reads_real_fields_at_a_nonzero_mb_address() {
        // Use a fresh ROM per test (with_pi_dma's HOST state is thread-local
        // per test since each #[test] gets its own OS thread by default).
        // Non-uniform big-endian cart words at devAddr 0x40 so a flat
        // (non-swizzled) DMA, a wrong devAddr, or a truncated len all fail.
        let mut rom = vec![0u8; 0x1000];
        let dev_addr: u32 = 0x40;
        rom[0x40..0x44].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        rom[0x44..0x48].copy_from_slice(&[0x00, 0x00, 0x10, 0x60]); // 0x1060 -- DmaMgr's sentinel
        rom[0x48..0x4C].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        load_rom(rom);

        let mut rdram = vec![0u8; 0x10000];
        let mb_vram: u64 = 0x8000_2000; // a REAL, nonzero vram address
        let mb_offset = 0x2000usize;

        // OSIoMesg fields at mb_offset + {retQueue +0x4, dramAddr +0x8,
        // devAddr +0xC, size +0x10} -- native byte order, DmaMgr's real
        // layout (0x08-byte OSIoMesgHdr).
        let dram_target_vram: u32 = 0x8000_5000;
        let size: u32 = 0xC; // 3 words -- non-zero, multi-word
        rdram[mb_offset + 0x4..mb_offset + 0x8].copy_from_slice(&0u32.to_ne_bytes()); // no retQueue
        rdram[mb_offset + 0x8..mb_offset + 0xC].copy_from_slice(&dram_target_vram.to_ne_bytes());
        rdram[mb_offset + 0xC..mb_offset + 0x10].copy_from_slice(&dev_addr.to_ne_bytes());
        rdram[mb_offset + 0x10..mb_offset + 0x14].copy_from_slice(&size.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r5 = mb_vram;
        ctx.r6 = 0; // OS_READ / ToRdram
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        // dramAddr (0x8000_5000) -> rdram offset 0x5000. Each big-endian
        // cart word must arrive so the guest's MEM_W reads it intact; rdram
        // is native-word storage, so physical bytes are byte-reversed. A
        // wrong offset would read size=0 (delivering nothing) or the wrong
        // devAddr; a flat copy would byte-reverse the words.
        let w0 = u32::from_ne_bytes(rdram[0x5000..0x5004].try_into().unwrap());
        let w1 = u32::from_ne_bytes(rdram[0x5004..0x5008].try_into().unwrap());
        let w2 = u32::from_ne_bytes(rdram[0x5008..0x500C].try_into().unwrap());
        assert_eq!(w0, 0x1234_5678, "first ROM word must be delivered intact");
        assert_eq!(
            w1, 0x0000_1060,
            "second word (DmaMgr's 0x1060 sentinel) proves the full size was read, not 0/one word"
        );
        assert_eq!(w2, 0xDEAD_BEEF, "third word confirms the exact len (0xC)");
        // And nothing spilled past the declared length.
        let after = u32::from_ne_bytes(rdram[0x500C..0x5010].try_into().unwrap());
        assert_eq!(after, 0, "DMA must not write past size (0xC bytes)");
    }

    /// Regression test for the OoT-boot hang (2026-07-14): `osEPiReadIo`
    /// delivered the cartridge word into rdram FLAT, but the guest reads
    /// individual bytes back through `MEM_BU`'s `^3` byte-lane XOR (rdram is
    /// native-endian-word storage). `Locale_Init` DMAs the ROM header, `lbu`s
    /// the region byte, accepts only 'E'/'J', else `LogUtils_HungupThread`s.
    /// A flat copy delivered the wrong byte -> neither-E-nor-J -> deliberate
    /// hang. This models that exact read with a distinguishable word so a
    /// regression to flat semantics fails here, not 8 frames into a boot.
    #[test]
    fn os_epi_read_io_word_reads_back_through_mem_bu_unswapped() {
        // ROM word at devAddr 0x3C = `5A 4C 4A 00` (OoT's real `Z L J \0`);
        // guest wants MEM_BU(dram+2) == 0x4A ('J').
        let mut rom = vec![0u8; 0x100];
        rom[0x3C..0x40].copy_from_slice(&[0x5A, 0x4C, 0x4A, 0x00]);
        load_rom(rom);

        let mut rdram = vec![0u8; 0x1000];
        let dram_vram: u64 = 0x8000_0024;
        let dram_off = 0x24usize;

        let mut ctx = ctx_zeroed();
        ctx.r5 = 0x3C; // devAddr
        ctx.r6 = dram_vram; // dramAddr
        unsafe { osEPiReadIo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        // MEM_BU(dram_off ^ 3) is the guest's byte read; +2 must be 'J'.
        assert_eq!(rdram[dram_off ^ 3], 0x5A); // 'Z'
        assert_eq!(rdram[(dram_off + 2) ^ 3], 0x4A); // 'J' -- the region byte
                                                     // And MEM_W reads the cart word intact (native-endian word storage).
        let w = u32::from_ne_bytes(rdram[dram_off..dram_off + 4].try_into().unwrap());
        assert_eq!(w, 0x5A4C_4A00);
    }

    /// Regression test for the SRAM-DMA-treated-as-ROM crash (2026-07-15):
    /// OoT's `Sram_InitSram -> SsSram_ReadWrite -> SsSram_Dma` issues a PI DMA
    /// with `devAddr = 0x08000000` (PI_DOM2_ADDR2, the SRAM cartridge base --
    /// rcp.h:714), which the old `osEPiStartDma_recomp` blindly read from the
    /// ROM image -> `InMemoryRom::read_into` past the 55MB ROM -> loud trap.
    /// The fix routes domain-2 devAddrs to the registered `SaveStorage`.
    ///
    /// Drives the REAL raw-pointer shim path (not `PiDma::start_dma`) for both
    /// directions: build an OSIoMesg exactly as `SsSram_Dma` does (dramAddr
    /// +0x8, devAddr +0xC, size +0x10, per pi.h:52-58), OS_WRITE the pattern to
    /// SRAM, then OS_READ it back into a different rdram region and assert the
    /// guest's own `MEM_BU`/`MEM_W` accessors read every byte in the SAME
    /// order. A flat (non-swizzled) copy in either direction fails here.
    #[test]
    fn os_epi_start_dma_round_trips_sram_save_domain() {
        // A ROM whose bytes at offset 0 are DISTINCT from the SRAM pattern, so
        // a regression that reads the ROM instead of the save is caught.
        let mut rom = vec![0u8; 0x1000];
        rom[0..4].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        load_rom(rom);
        // OoT uses 32 KiB banked SRAM.
        set_save(Box::new(fn64_runtime::InMemorySaveStorage::for_device(
            fn64_runtime::SaveType::SramBanked,
        )));

        let mut rdram = vec![0u8; 0x10000];
        let mb_offset = 0x2000usize;
        let mb_vram: u64 = 0x8000_2000;
        let sram_dev_addr: u32 = 0x0800_0010; // domain-2 base + 0x10
        let size: u32 = 8;

        // Guest lays 8 distinct bytes at rdram 0x5000 via MEM_BU (byte-lane
        // `^3`), the way it would build a save record before writing it out.
        let src = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let src_off = 0x5000usize;
        for (k, &b) in src.iter().enumerate() {
            rdram[(src_off + k) ^ 3] = b;
        }
        // OSIoMesg for the WRITE (OS_WRITE=1 -> FromRdram).
        rdram[mb_offset + 0x4..mb_offset + 0x8].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb_offset + 0x8..mb_offset + 0xC].copy_from_slice(&0x8000_5000u32.to_ne_bytes());
        rdram[mb_offset + 0xC..mb_offset + 0x10].copy_from_slice(&sram_dev_addr.to_ne_bytes());
        rdram[mb_offset + 0x10..mb_offset + 0x14].copy_from_slice(&size.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r5 = mb_vram;
        ctx.r6 = 1; // OS_WRITE
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        // OSIoMesg for the READ back into a DIFFERENT region (0x6000).
        let dst_off = 0x6000usize;
        rdram[mb_offset + 0x8..mb_offset + 0xC].copy_from_slice(&0x8000_6000u32.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r5 = mb_vram;
        ctx.r6 = 0; // OS_READ
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        // Guest reads readBuff[k] via MEM_BU((dst)+k) = rdram[(dst+k)^3];
        // every byte must match the original -- swizzle cancels round-trip.
        for (k, &b) in src.iter().enumerate() {
            assert_eq!(
                rdram[(dst_off + k) ^ 3],
                b,
                "SRAM round-trip byte {k}: save DMA must route to the save store, \
                 word-swizzled, not the ROM"
            );
        }
        // The ROM byte at offset 0 (0xAA) must NOT appear -- proves the read
        // hit the save store, not the ROM image.
        assert_ne!(rdram[dst_off ^ 3], 0xAA);
    }

    /// Regression test for the real infinite-loop bug `examples/wm2000-boot`
    /// surfaced (2026-07-14): `osEPiStartDma_recomp` never wrote `ctx.r2`
    /// ($v0), so NWXE's chunked-DMA caller (`func_80000660`, asm
    /// 0x800006E4-0x800006FC: `bne $v0, $zero, L_800006E4`) read whatever
    /// stale value `r2` already held and looped forever instead of falling
    /// through to `osRecvMesg`. Seed `ctx.r2` with a realistic STALE
    /// NON-ZERO value beforehand (mirroring the real caller's register
    /// state at the call site) so a regression that stops writing `ctx.r2`
    /// would fail this test even though a zero-initialized `ctx` would
    /// have hidden the bug.
    #[test]
    fn os_epi_start_dma_writes_zero_return_value_even_with_stale_nonzero_r2() {
        load_rom(vec![0xCDu8; 0x1000]);

        let mut rdram = vec![0u8; 0x10000];
        let mb_offset = 0x2000usize;
        // DmaMgr's real OSIoMesg layout: retQueue +0x4, dramAddr +0x8,
        // devAddr +0xC, size +0x10 (0x08-byte OSIoMesgHdr).
        rdram[mb_offset + 0x4..mb_offset + 0x8].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb_offset + 0x8..mb_offset + 0xC].copy_from_slice(&0x8000_5000u32.to_ne_bytes());
        rdram[mb_offset + 0xC..mb_offset + 0x10].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb_offset + 0x10..mb_offset + 0x14].copy_from_slice(&4u32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r5 = 0x8000_2000;
        ctx.r6 = 0; // OS_READ / ToRdram
        ctx.r2 = 0x1234; // stale non-zero, as a real caller's register would hold
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(
            ctx.r2, 0,
            "osEPiStartDma_recomp must overwrite $v0 with 0 (success) on every \
             synchronous-completion path, or NWXE's chunked-DMA retry loop spins forever"
        );
    }

    #[test]
    fn os_epi_start_dma_without_a_loaded_rom_is_a_loud_named_trap() {
        assert_subprocess_aborts("pi::tests::__os_epi_start_dma_no_rom_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __os_epi_start_dma_no_rom_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            // mb points at an all-zero rdram region -> ret_queue==0 (no
            // completion post attempted), dev_addr==0, len==0 -- the load-
            // bearing assertion here is that with_pi_dma panics because no
            // ROM was ever installed in this fresh subprocess, not that the
            // (deliberately trivial) transfer parameters are realistic.
            let mut ctx = ctx_zeroed();
            let mut rdram = vec![0u8; 64];
            ctx.r5 = 0; // mb address 0
            ctx.r6 = 0; // direction = ToRdram
            unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        }
    }
}
