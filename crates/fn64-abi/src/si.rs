use super::*;

/// `__osSiRawStartDma(s32 direction, u8* dramAddr)` -- `a0`=direction
/// (`ctx->r4`; per the public libultra manual and this milestone's real
/// call-site evidence below, `1` = "write this PIF command block from rdram
/// TO the PIF, execute it, and read the response back into the same
/// buffer" -- the SI-manager's synchronous raw-transfer primitive
/// underlying `osContStartQuery`/`osContStartReadData`), `a1`=dramAddr
/// (`ctx->r5`, the PIF command-block buffer's rdram address).
///
/// ## What this really does (this wave, replacing the prior loud trap)
///
/// A real call site (`funcs_15.c` asm 0x80036040-0x80036064, the function
/// this milestone's evidence shows builds a controller-probe PIF block)
/// writes a standard libultra PIF-RAM command block into the buffer before
/// calling this: byte 0 = tx-size (`0xFF` = end-of-block marker observed at
/// offsets 0x26/final), each channel's header is
/// `[tx_size, rx_size, cmd, ...tx_bytes]` followed by `rx_size` response
/// bytes to fill in -- the public libultra manual's documented PIF-RAM
/// protocol (`osContStartQuery`'s `0x01,0x03` 3-byte-tx-then-3-byte-rx
/// status-query shape, `osContStartReadData`'s 1-byte-tx/4-byte-rx
/// read-data shape). This function walks channels 0-3 in that documented
/// format, filling each channel's response bytes from `PifModel`
/// (`fn64_runtime::si` -- "one standard controller on port 0, no pak, ports
/// 1-3 absent," per the task's explicit scope) rather than a fabricated
/// byte pattern, and stops at the first `0xFF` tx-size byte (the documented
/// end-of-block marker) or buffer exhaustion.
///
/// Completion is posted through `OS_EVENT_SI` (5, per the public libultra
/// manual's event-code table) via the SAME `Executor::inject_event` path
/// every other completion source uses -- matching `docs/DESIGN.md`
/// section 2's "closing the asymmetry" design point. If no
/// `osSetEventMesg(5, ...)` registration exists yet (this call happening
/// before the game registers its SI event), the post is silently absent
/// (mirrors `advance_time`'s VI-retrace handling of the same
/// not-yet-registered case) rather than panicking -- the DMA itself still
/// completes and the response bytes are still written, matching real
/// hardware where the SI interrupt fires regardless of whether software
/// has hooked it yet.
///
/// Real-hardware commands this milestone's `PifModel` does NOT model
/// (EEPROM/mempak read-write commands, reset) are represented as
/// `CONT_ABSENT`-shaped responses per channel walked, which is honest for
/// "no such device," not a guessed success.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osSiRawStartDma_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let dram_addr = RdramAddr::from_gpr(ctx.r5);
    let base = dram_addr.offset() as usize;

    with_executor(|exec| {
        let pif = *exec.pif();
        let mut port = 0usize;
        let mut cursor = base;
        // Documented PIF-RAM command-block format: walk channel headers
        // until the 0xFF end-of-block marker or a bogus/oversized read that
        // would run off any sane buffer (64 bytes is PIF RAM's own real
        // hardware size, per public documentation -- used here only as a
        // runaway guard, not asserted as this buffer's actual allocation).
        for _ in 0..16 {
            let tx_size = unsafe { *rdram.add(cursor) };
            if tx_size == 0xFF {
                break;
            }
            let rx_size = unsafe { *rdram.add(cursor + 1) };
            if tx_size == 0 && rx_size == 0 {
                cursor += 1;
                continue;
            }
            let cmd = unsafe { *rdram.add(cursor + 2) };
            let rx_off = cursor + 2 + tx_size as usize;
            match (cmd, rx_size) {
                // osContStartQuery-shape: 1-byte tx (the 0xFF query command
                // itself is tx_size/cmd, not a separate byte in some
                // encodings; this crate matches on the documented 3-byte
                // status response regardless of the exact tx encoding
                // variant, since PifModel's response doesn't depend on it).
                (_, 3) => {
                    let resp = pif.query_response(port);
                    unsafe {
                        std::ptr::copy_nonoverlapping(resp.as_ptr(), rdram.add(rx_off), 3);
                    }
                }
                (_, 4) => {
                    let resp = pif.read_data_response(port);
                    unsafe {
                        std::ptr::copy_nonoverlapping(resp.as_ptr(), rdram.add(rx_off), 4);
                    }
                }
                _ => {
                    // Unmodeled command shape for this milestone (see doc
                    // comment) -- leave whatever bytes were already there
                    // rather than fabricating a response with no documented
                    // basis.
                }
            }
            cursor = rx_off + rx_size as usize;
            port += 1;
        }
    });

    const OS_EVENT_SI: u32 = 5;
    with_executor(|exec| {
        if exec.event_table_contains(OS_EVENT_SI) {
            exec.inject_event(ExternalEvent::OsEvent(OS_EVENT_SI));
        }
    });
}

/// Host-facing input seam: feed controller `port`'s live button/stick state
/// so the game's next `osContGetReadData` reflects it. `buttons` is the N64
/// `OSContPad.button` bitmask (`oot-decomp/include/controller.h:4-17`:
/// `BTN_A = 0x8000`, `BTN_B = 0x4000`, `BTN_Z = 0x2000`, `BTN_START = 0x1000`,
/// d-pad `0x0800..0x0100`, `BTN_L = 0x0020`, `BTN_R = 0x0010`, C-buttons
/// `0x0008..0x0001`); `stick_x`/`stick_y` are the signed analog values
/// (`OSContPad.stick_x`/`stick_y`, centered at 0). A scripted-input harness
/// (`examples/oot-boot`) calls this to drive OoT headlessly. Idle by default,
/// so an un-driven boot sees an honest neutral pad.
pub fn set_controller_state(port: usize, buttons: u16, stick_x: i8, stick_y: i8) {
    let input = fn64_runtime::si::ContInput {
        button: buttons,
        stick_x,
        stick_y,
    };
    with_executor(|exec| exec.set_controller_input(port, input));
}

/// `osContGetQuery(OSContStatus *data)` -- ONE argument, `a0`=data
/// (`ctx->r4`), returns void. Byte-verified against the OoT decomp
/// (`oot-decomp/src/libultra/io/contquery.c:31`,
/// `void osContGetQuery(OSContStatus* data)`) and its real call site
/// `PadSetup_Init` (`oot-decomp/src/libu64/padsetup.c:19`,
/// `osContGetQuery(status)` where `status = padMgr->padStatus`, an
/// `OSContStatus[MAXCONTROLLERS]` array). The generated call site confirms
/// the shape: `funcs_55.c:2193` sets only `$a0` (`ctx->r4 = ctx->r16`, the
/// `padStatus` pointer) and leaves `$a1` UNSET -- a prior wave's
/// `(int channel, OSContStatus* data)` signature read the data pointer from
/// the stale `$a1`/`ctx->r5` (garbage left by the preceding `osRecvMesg`
/// whose asm 0x800CD438 sets `$a1 = 0`), then dereferenced it: a real
/// EXC_BAD_ACCESS deep in `Main -> PadMgr_Init -> PadSetup_Init` on OoT's
/// first controller-status probe, which is why boot never yielded again
/// after DmaMgr delivered the code-segment DMA (thread 3 died mid-C, before
/// the next shim/yield).
///
/// Fills the whole `OSContStatus[MAXCONTROLLERS]` array, one entry per port
/// (`__osContGetInitData`, `oot-decomp/src/libultra/io/controller.c:58`,
/// iterates all `__osMaxControllers` and advances `data++` each). Each
/// 4-byte entry is `{type: u16 @0, status: u8 @2, errno: u8 @3}`
/// (`oot-decomp/include/ultra64/controller.h:121`). The game reads these
/// back with `MEM_HU`/`MEM_BU` (`funcs_55.c:2205/2214`:
/// `MEM_BU(reg,3)`=errno, `MEM_HU(reg,0)`=type, compared `== 0x0005`
/// = `CONT_TYPE_NORMAL`), whose `^2`/`^3` sub-word swizzle
/// (`refs/N64RecompSource/include/recomp.h:104-108`) requires each logical
/// N64 struct byte at struct-offset `o` to live in the host buffer at
/// `(base + o) ^ 3` -- so a present port-0 standard controller must read
/// `type == 0x0005, errno == 0`, and absent ports 1-3 must read a non-zero
/// `errno` (`CONT_NO_RESPONSE_ERROR = 0x08`,
/// `oot-decomp/include/ultra64/controller.h:66`, the value
/// `CHNL_ERR(no-response) = (0x80 >> 4)` yields) so `PadSetup_Init`'s
/// `switch (status[i].errno)` skips them.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContGetQuery_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let data_addr = RdramAddr::from_gpr(ctx.r4).offset() as usize;
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let pif = with_executor(|exec| *exec.pif());
    for port in 0..MAXCONTROLLERS {
        let resp = pif.query_response(port);
        let absent = (resp[2] & fn64_runtime::si::CONT_ABSENT) != 0;
        // `query_response` returns the PIF wire bytes `[typeh, typel,
        // status]`. `__osContGetInitData` assembles the game-visible
        // `OSContStatus.type` u16 as `typel << 8 | typeh`
        // (`controller.c:72`), i.e. `(resp[1] << 8) | resp[0]` -- so a
        // standard controller (`[0x05, 0x00, ..]`) becomes `0x0005 =
        // CONT_TYPE_NORMAL`, which is what `PadSetup_Init` compares against.
        // The 4 logical N64 struct bytes are `type` (u16, big-endian: hi @0,
        // lo @1), `status` @2, `errno` @3. An absent port reports no-response
        // in `errno` with type/status left zero (matching the decomp's
        // `if (data->errno) continue;`, which never writes them on error).
        let type_u16: u16 = ((resp[1] as u16) << 8) | resp[0] as u16;
        let entry: [u8; 4] = if absent {
            [0, 0, 0, CONT_NO_RESPONSE_ERROR]
        } else {
            [(type_u16 >> 8) as u8, (type_u16 & 0xFF) as u8, resp[2], 0]
        };
        let base = data_addr + port * 4;
        // Store each logical byte at its `^3`-swizzled host position so the
        // game's MEM_HU/MEM_BU reads (recomp.h) recover the right values --
        // see the doc comment above for the byte-order derivation.
        for (o, &b) in entry.iter().enumerate() {
            unsafe {
                storage.write_u8(
                    RdramAddr::from_offset(
                        u32::try_from(base + o).expect("OSContStatus RDRAM address exceeds u32"),
                    ),
                    b,
                );
            }
        }
    }
}

/// `CONT_NO_RESPONSE_ERROR` (`oot-decomp/include/ultra64/controller.h:66`):
/// the `OSContStatus.errno` value an absent/non-responding controller port
/// reports (`CHNL_ERR` of a PIF no-response = `(CHNL_ERR_NORESP=0x80) >> 4`).
const CONT_NO_RESPONSE_ERROR: u8 = 0x08;

/// `MAXCONTROLLERS` (`oot-decomp/include/ultra64/controller.h:9`): the N64
/// has four controller ports; `osContGetQuery` fills one `OSContStatus` per
/// port.
const MAXCONTROLLERS: usize = 4;

/// `osContGetReadData(OSContPad *pad) -> void` -- `a0`=`ctx->r4`, the base of
/// an `OSContPad[MAXCONTROLLERS]` array (`padMgr->pads`, decomp
/// `oot-decomp/src/code/padmgr.c:364` `osContGetReadData(padMgr->pads)`).
/// This is the INPUT SEAM's game-facing half: the per-port button/stick state
/// a host harness fed via `PifModel::set_input` lands in the pad array the
/// game reads each retrace to drive Link.
///
/// ## OSContPad layout + swizzle (byte-cited)
///
/// `oot-decomp/include/ultra64/controller.h:127-132`:
/// `{ button: u16 @0x00, stick_x: s8 @0x02, stick_y: s8 @0x03, errno: u8 @0x04 }`,
/// `size = 0x06`. The decomp `osContGetReadData`
/// (`oot-decomp/src/libultra/io/contreaddata.c:22`) iterates all
/// `__osMaxControllers`, sets `errno = CHNL_ERR(read)` for each, and ONLY
/// fills `button`/`stick_x`/`stick_y` when `errno == 0` -- so a present
/// controller reports `errno == 0` + live input, an absent port reports a
/// nonzero `errno` (`CONT_NO_RESPONSE_ERROR = 0x08`) with the game leaving the
/// stale button/stick (padmgr then `bzero`s pads[1]/pads[3] anyway).
///
/// The game reads these fields back through the recomp memory macros
/// (`refs/N64RecompSource/include/recomp.h:104-108`): `button` via `MEM_HU`
/// (`^2` halfword swizzle), `stick_x`/`stick_y`/`errno` via `MEM_B`/`MEM_BU`
/// (`^3` byte swizzle). Storing each LOGICAL struct byte at host offset
/// `(base + o) ^ 3` satisfies both: the two bytes of the big-endian `button`
/// u16 land at `0^3 = 3` (hi) and `1^3 = 2` (lo), so a native `MEM_HU` read at
/// `0^2 = 2` recovers `hi<<8 | lo` -- identical to the `^3` per-byte store
/// `osContGetQuery_recomp` already uses for `OSContStatus`. A flat
/// (unswizzled) copy, which a prior WIP did, put every field at the wrong lane
/// and the game saw garbage/no input -- the exact fail this shim's test pins.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContGetReadData_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let base_addr = RdramAddr::from_gpr(ctx.r4).offset() as usize;
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let pif = with_executor(|exec| *exec.pif());
    // Diagnostic (opt-in via FN64_TRACE_CONT): proves PadMgr actually polls
    // input, and echoes what port-0 state the game is about to see -- the
    // observable evidence a scripted press reaches the game.
    if std::env::var_os("FN64_TRACE_CONT").is_some() {
        let p0 = pif.read_data_response(0);
        eprintln!(
            "[fn64-abi] osContGetReadData(pad@{base_addr:#x}): port0 button={:#06x} stick=({},{})",
            u16::from_be_bytes([p0[0], p0[1]]),
            p0[2] as i8,
            p0[3] as i8,
        );
    }
    for port in 0..MAXCONTROLLERS {
        // A present standard controller reports errno == 0 and its live input;
        // an absent port reports no-response, matching the decomp's
        // `errno = CHNL_ERR(read)` branch (button/stick left zero here).
        let absent = (pif.query_response(port)[2] & fn64_runtime::si::CONT_ABSENT) != 0;
        // `read_data_response` is the `[button_hi, button_lo, stick_x, stick_y]`
        // PIF wire shape filled from the fed input (idle default).
        let resp = pif.read_data_response(port);
        // Assemble the 6-byte OSContPad in LOGICAL struct-offset order:
        // button hi@0, button lo@1, stick_x@2, stick_y@3, errno@4, pad@5.
        let (button_hi, button_lo, stick_x, stick_y, errno) = if absent {
            (0, 0, 0, 0, CONT_NO_RESPONSE_ERROR)
        } else {
            (resp[0], resp[1], resp[2], resp[3], 0)
        };
        let pad: [u8; 6] = [button_hi, button_lo, stick_x, stick_y, errno, 0];
        let base = base_addr + port * 6;
        // Store each logical byte at its `^3`-swizzled host position so the
        // game's MEM_HU(button)/MEM_BU(stick,errno) reads recover the right
        // values -- see the doc comment for the byte-order derivation.
        for (o, &b) in pad.iter().enumerate() {
            unsafe {
                storage.write_u8(
                    RdramAddr::from_offset(
                        u32::try_from(base + o).expect("OSContPad RDRAM address exceeds u32"),
                    ),
                    b,
                );
            }
        }
    }
    // osContGetReadData returns void; leave $v0 as the decomp does (unset).
}

/// `osContInit(OSMesgQueue *mq, u8 *bitpattern, OSContStatus *data) -> s32`
/// -- `a0`=mq (`ctx->r4`), `a1`=bitpattern (`ctx->r5`), `a2`=data
/// (`ctx->r6`). Public libultra manual's documented one-time controller-
/// manager bring-up: probes all 4 ports and sets one bit per populated
/// port in `*bitpattern`. Function-table slot only
/// (`recomp_overlays.inl:2918`), reached from `PadMgr_Init`
/// (BOOT-PLAN.md rung 15's forcing-function call) -- implemented for real
/// against `PifModel`'s "port 0 populated, 1-3 absent" model
/// (`si.rs`'s module doc, this task's explicit scope).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContInit_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let bitpattern_addr = RdramAddr::from_gpr(ctx.r5).offset() as usize;
    let data_addr = RdramAddr::from_gpr(ctx.r6).offset() as usize;
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let mut mask: u8 = 0;
    with_executor(|exec| {
        let pif = *exec.pif();
        for port in 0..4usize {
            let resp = pif.query_response(port);
            let absent = (resp[2] & fn64_runtime::si::CONT_ABSENT) != 0;
            if !absent {
                mask |= 1 << port;
            }
            // Write each OSContStatus entry SWIZZLED (`^3`), exactly like
            // osContGetQuery_recomp -- the game reads type/status/errno back
            // via MEM_HU/MEM_BU (recomp.h), so flat stores would transpose
            // them. type u16 = (resp[1]<<8)|resp[0] (controller.c:72);
            // absent ports report no-response in errno with type/status 0.
            let type_u16: u16 = ((resp[1] as u16) << 8) | resp[0] as u16;
            let entry: [u8; 4] = if absent {
                [0, 0, 0, CONT_NO_RESPONSE_ERROR]
            } else {
                [(type_u16 >> 8) as u8, (type_u16 & 0xFF) as u8, resp[2], 0]
            };
            let base = data_addr + port * 4;
            for (o, &b) in entry.iter().enumerate() {
                unsafe {
                    storage.write_u8(
                        RdramAddr::from_offset(
                            u32::try_from(base + o)
                                .expect("OSContStatus RDRAM address exceeds u32"),
                        ),
                        b,
                    );
                }
            }
        }
    });
    unsafe {
        // ctlBitfield is a `u8*`: the decomp writes a SINGLE byte
        // `*ctlBitfield = bits` (controller.c:96), bits<=0x0F. Write one
        // swizzled byte (^3). A second byte at +1 would (a) be always 0 for a
        // u16 hi-byte and (b) clobber the adjacent variable -- and the flat
        // +0 store misses the swizzled sentinel address PadSetup_Init checks
        // (funcs_55.c 0x800CD414 `bnel $t7,0xFF`), so it would bail and skip
        // all controller-present stores.
        storage.write_u8(
            RdramAddr::from_offset(
                u32::try_from(bitpattern_addr)
                    .expect("controller bitpattern RDRAM address exceeds u32"),
            ),
            mask,
        );
    }
    ctx.r2 = 0;
}

/// `osContSetCh(u8 ch) -> s32` -- `a0`=`ctx->r4`. Public libultra manual:
/// restricts subsequent controller-manager polling to the first `ch`
/// channels. This crate's `PifModel` always reports the same fixed 4-port
/// state regardless of channel count (`si.rs`'s module doc: "one standard
/// controller on port 0... ports 1-3 absent" is not parameterized by a
/// runtime channel-count setting) -- stored as plain host state for
/// fidelity/logging, with no other behavioral effect, matching
/// `osAiSetFrequency_recomp`'s existing "store it, no consumer needs it
/// yet" pattern for an unconsumed configuration value. Function-table slot
/// only (`recomp_overlays.inl:2958`), reached from `PadMgr_Init`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContSetCh_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    CONT_CHANNELS.with(|cell| cell.set((ctx.r4 & 0xFF) as u8));
    ctx.r2 = 0;
}

thread_local! {
    static CONT_CHANNELS: Cell<u8> = const { Cell::new(4) };
}

/// `osContStartQuery(OSMesgQueue *mq) -> s32` -- `a0`=`ctx->r4`. Public
/// libultra manual: kicks off an async PIF status-query DMA, posting
/// completion to `mq`. This crate's PI/SI DMA is synchronous-modeled
/// throughout (`__osSiRawStartDma_recomp`'s doc comment: "every path... is
/// success"/completes immediately) -- consistent with that, this shim
/// posts the `OS_EVENT_SI` completion (mirroring
/// `__osSiRawStartDma_recomp`'s own event-post at the bottom of this file)
/// immediately rather than modeling a real async gap, since no evidence
/// shows any call site depending on a delay here. Function-table slot only
/// (`recomp_overlays.inl:2933`), reached from `PadMgr_Init`/its polling
/// thread body (BOOT-PLAN.md rung 15).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContStartQuery_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    const OS_EVENT_SI: u32 = 5;
    with_executor(|exec| {
        exec.set_event_mesg(OS_EVENT_SI, mq_addr, 0);
        exec.inject_event(ExternalEvent::OsEvent(OS_EVENT_SI));
    });
    ctx.r2 = 0;
}

/// `osContStartReadData(OSMesgQueue *mq) -> s32` -- same shape/reasoning as
/// `osContStartQuery_recomp` (Public libultra manual's paired async
/// button/stick-read DMA kickoff). Function-table slot only
/// (`recomp_overlays.inl:2919`), reached from PadMgr's polling thread body.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContStartReadData_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    const OS_EVENT_SI: u32 = 5;
    with_executor(|exec| {
        exec.set_event_mesg(OS_EVENT_SI, mq_addr, 0);
        exec.inject_event(ExternalEvent::OsEvent(OS_EVENT_SI));
    });
    ctx.r2 = 0;
}

/// `osMotorInit(OSMesgQueue *mq, OSPfs *pfs, int channel) -> s32` --
/// Rumble Pak initialization. Zero real call sites in this corpus and
/// BOOT-PLAN.md's own "rumble-pak specific... not required for a picture
/// on screen" note. `PifModel` (`si.rs`) explicitly models "no pak" on
/// every port (this task's stated scope) -- loud-trapped rather than
/// fabricating a fake accessory-present/success response, since a real
/// game branching on this return value deserves a named failure, not a
/// silently-wrong "rumble pak found."
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osMotorInit_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "osMotorInit_recomp: no Rumble Pak modeled (PifModel's explicit 'no pak' scope, \
         si.rs's module doc) and no real call site in games/OOTU/RecompiledFuncs exercises \
         this on the boot path (BOOT-PLAN.md: 'not required for a picture on screen') -- a \
         fabricated success/failure response would be an unearned guess."
    );
}

/// `__osMotorAccess(OSPfs *pfs, int accesslib)` -- Rumble Pak channel-access
/// mutex primitive, reached only from PadMgr's deeper controller-pak
/// polling (BOOT-PLAN.md: after `osContStartQuery` succeeds; not required
/// for a picture on screen). Zero real call sites in this corpus
/// (function-table slot only, `recomp_overlays.inl:2916`). Same
/// no-accessory-modeled reasoning as `osMotorInit_recomp` -- loud trap.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osMotorAccess_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "__osMotorAccess_recomp: no Rumble Pak modeled (see osMotorInit_recomp's doc comment) \
         and no real call site in games/OOTU/RecompiledFuncs exercises this on the boot path."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    /// Regression for the OoT-boot `PadSetup_Init` EXC_BAD_ACCESS: the real
    /// `osContGetQuery(OSContStatus* data)` takes its ONLY argument (the
    /// array pointer) in `$a0`/`ctx.r4`; the buggy prior signature read it
    /// from `$a1`/`ctx.r5`, which the real call site (`funcs_55.c:2193`)
    /// leaves as stale garbage, so the shim dereferenced a wild pointer.
    ///
    /// This test wires `r4` and `r5` to two DIFFERENT, both-valid rdram
    /// addresses and asserts the OSContStatus array lands at `r4`'s address
    /// (and that `r5`'s address is untouched) -- so reintroducing the bug
    /// (reading the pointer from `r5`) makes it fail rather than pass. It
    /// also checks all four ports are filled with the exact byte-swizzled
    /// values the game's own MEM_HU/MEM_BU reads recover: port 0 a present
    /// standard controller (`type == 0x0005 == CONT_TYPE_NORMAL`, `errno ==
    /// 0`), ports 1-3 absent (`errno == CONT_NO_RESPONSE_ERROR == 0x08`).
    #[test]
    fn os_cont_get_query_reads_array_pointer_from_a0_and_fills_all_ports() {
        // Fresh PIF state (default: port 0 standard, 1-3 absent).
        with_executor(|exec| *exec = fn64_runtime::Executor::new());

        let mut buf = vec![0u8; fn64_runtime::RDRAM_MMIO_WINDOW_END as usize];

        // Two distinct, both-valid vram addresses. r4 = the REAL data
        // pointer the game passes; r5 = a decoy the buggy shim would have
        // used. Kept 0x40 apart so the 0x10-byte (4 * OSContStatus) write
        // regions can't overlap.
        let data_vram: u64 = 0xFFFF_FFFF_8020_0000;
        let decoy_vram: u64 = 0xFFFF_FFFF_8020_0040;
        let data_off = RdramAddr::from_gpr(data_vram).offset() as usize;
        let decoy_off = RdramAddr::from_gpr(decoy_vram).offset() as usize;

        // Pre-poison the decoy region with a sentinel so "untouched" is a
        // real, checkable statement, not "happened to already be zero".
        for i in 0..0x10 {
            buf[decoy_off + i] = 0xAB;
        }

        let mut ctx = ctx_zeroed();
        ctx.r4 = data_vram;
        ctx.r5 = decoy_vram;
        unsafe { osContGetQuery_recomp(buf.as_mut_ptr(), &mut ctx as *mut _) };

        // Read each OSContStatus exactly as the generated game code does:
        // MEM_HU(base, 0) = *(u16*)(rdram + (base ^ 2)); MEM_BU(base, 3) =
        // *(u8*)(rdram + ((base + 3) ^ 3)) (recomp.h). Reading through the
        // same swizzle the reader uses is what makes this a faithful check
        // rather than an encoding of whatever byte order the writer chose.
        let read_type = |base: usize| -> u16 {
            let a = base ^ 2;
            u16::from_ne_bytes([buf[a], buf[a + 1]])
        };
        let read_errno = |base: usize| -> u8 { buf[(base + 3) ^ 3] };

        // Port 0: present standard controller.
        let p0 = data_off;
        assert_eq!(
            read_type(p0),
            0x0005,
            "port 0 type must read as CONT_TYPE_NORMAL (0x0005) via the game's MEM_HU"
        );
        assert_eq!(read_errno(p0), 0, "port 0 (present) has no channel error");

        // Ports 1-3: absent -> non-zero errno so PadSetup_Init skips them.
        for port in 1..4usize {
            let base = data_off + port * 4;
            assert_eq!(
                read_errno(base),
                0x08,
                "absent port {port} must report CONT_NO_RESPONSE_ERROR (0x08)"
            );
        }

        // The decoy region (r5's address) must be completely untouched --
        // proves the pointer came from r4, not r5. Under the old bug this
        // region would have been written (and r4's region left as zeros).
        for i in 0..0x10 {
            assert_eq!(
                buf[decoy_off + i],
                0xAB,
                "byte {i} at the r5/decoy address was written -- the shim read \
                 its pointer from the wrong register (the reintroduced bug)"
            );
        }
    }

    /// The INPUT-SEAM contract: a host harness feeds controller state via
    /// `set_controller_state`, and `osContGetReadData_recomp` writes it into
    /// the game's `OSContPad[MAXCONTROLLERS]` array at `$a0`/`ctx.r4`, in the
    /// exact byte-swizzled layout the game's own MEM_HU/MEM_BU reads recover.
    ///
    /// Fail-against-the-bug: it reads every field back through the SAME
    /// swizzle the recompiled game uses (`button` via MEM_HU `^2`, `stick`/
    /// `errno` via MEM_BU `^3`, recomp.h:104-108). A flat/unswizzled copy (the
    /// prior WIP) or a wrong button bit lands the bytes at the wrong lanes and
    /// this fails. It also checks the button HIGH byte carries `BTN_START`
    /// (0x1000) -- the scripted-boot press -- so an endianness flip fails too.
    #[test]
    fn os_cont_get_read_data_writes_swizzled_input_into_pad_array() {
        // Fresh state, then feed a distinctive input on port 0: Start+A held,
        // stick pushed. (BTN_A = 0x8000, BTN_START = 0x1000 -> 0x9000.)
        with_executor(|exec| *exec = fn64_runtime::Executor::new());
        set_controller_state(0, 0x9000, -50, 70);

        let mut buf = vec![0u8; fn64_runtime::RDRAM_MMIO_WINDOW_END as usize];
        let pad_vram: u64 = 0xFFFF_FFFF_8020_0000;
        let pad_off = RdramAddr::from_gpr(pad_vram).offset() as usize;

        let mut ctx = ctx_zeroed();
        ctx.r4 = pad_vram;
        unsafe { osContGetReadData_recomp(buf.as_mut_ptr(), &mut ctx as *mut _) };

        // Read each OSContPad field EXACTLY as the recompiled game does:
        // button via MEM_HU (`^2` halfword), the s8/u8 fields via MEM_BU
        // (`^3` byte). OSContPad size = 0x06 (controller.h:132).
        let read_button = |base: usize| -> u16 {
            let a = base ^ 2;
            u16::from_ne_bytes([buf[a], buf[a + 1]])
        };
        let read_i8 = |base: usize, o: usize| -> i8 { buf[(base + o) ^ 3] as i8 };
        let read_u8 = |base: usize, o: usize| -> u8 { buf[(base + o) ^ 3] };

        // Port 0: present -> errno 0 and the exact fed input.
        let p0 = pad_off;
        assert_eq!(
            read_button(p0),
            0x9000,
            "port 0 button must read back BTN_A|BTN_START (0x9000) via the game's MEM_HU"
        );
        assert_ne!(
            read_button(p0) & 0x1000,
            0,
            "BTN_START (0x1000) must be set -- the scripted press must reach the game"
        );
        assert_eq!(read_i8(p0, 2), -50, "stick_x");
        assert_eq!(read_i8(p0, 3), 70, "stick_y");
        assert_eq!(read_u8(p0, 4), 0, "port 0 (present) errno == 0");

        // Ports 1-3: absent -> nonzero errno so the game ignores them.
        for port in 1..MAXCONTROLLERS {
            let base = pad_off + port * 6;
            assert_eq!(
                read_u8(base, 4),
                CONT_NO_RESPONSE_ERROR,
                "absent port {port} errno must be CONT_NO_RESPONSE_ERROR (0x08)"
            );
            assert_eq!(read_button(base), 0, "absent port {port} button zeroed");
        }
    }

    /// `__osSiRawStartDma_recomp` is real this wave (replacing the prior
    /// loud trap) -- verifies a port-0 status-query channel (tx_size=1,
    /// rx_size=3) gets `PifModel::query_response(0)`'s real bytes written
    /// back, and that an absent port (1) gets `CONT_ABSENT` set.
    #[test]
    fn os_si_raw_start_dma_fills_real_pif_query_responses() {
        let mut rdram = vec![0u8; 64];
        // Channel 0: tx_size=1, rx_size=3, cmd=0xFF (query), 1 tx byte, then
        // 3 response bytes to be filled at offset 3..6.
        rdram[0] = 1;
        rdram[1] = 3;
        rdram[2] = 0xFF;
        rdram[3] = 0; // the 1 tx byte
                      // rdram[4..7] is the response area for this channel (rx_off = 0+2+1=3,
                      // so response bytes land at 3..6 -- recompute: cursor=0, tx_size=1,
                      // rx_off = cursor+2+tx_size = 0+2+1 = 3, filled 3..6).
                      // Channel 1 starts at cursor = rx_off + rx_size = 3+3 = 6.
        rdram[6] = 1; // tx_size
        rdram[7] = 3; // rx_size
        rdram[8] = 0xFF; // cmd
        rdram[9] = 0; // tx byte
                      // response area for channel 1: rx_off = 6+2+1=9, filled 9..12.
        rdram[12] = 0xFF; // end-of-block marker, channel 2 onward absent

        let mut ctx = ctx_zeroed();
        ctx.r5 = 0x8000_0000; // dramAddr vram -> rdram offset 0
        unsafe { __osSiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        // Port 0: standard controller, no pak, not absent.
        assert_eq!(&rdram[3..6], &[0x05, 0x00, 0x00]);
        // Port 1: absent bit set.
        assert_eq!(
            rdram[9 + 2] & fn64_runtime::CONT_ABSENT,
            fn64_runtime::CONT_ABSENT
        );
    }

    /// osContInit: (1) OSContStatus entries must be written SWIZZLED (^3) like
    /// osContGetQuery, and (2) ctlBitfield is a `u8*` -- a SINGLE swizzled
    /// byte, no +1 store. Fails against the bug (flat status stores + two
    /// bitfield bytes at flat +0/+1).
    #[test]
    fn os_cont_init_swizzles_status_and_writes_single_bitfield_byte() {
        // data at offset 0x40 (16 bytes = 4 OSContStatus), bitfield at 0x80.
        let mut rdram = vec![0xEEu8; 256]; // 0xEE sentinel: catch stray writes.
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0; // mq (unused for the byte layout under test)
        ctx.r5 = 0x8000_0080; // ctlBitfield
        ctx.r6 = 0x8000_0040; // data (OSContStatus[4])
        unsafe { osContInit_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        // Port 0 is a standard controller (type 0x0005). The swizzled entry
        // [type_hi=0x00, type_lo=0x05, status=0x00, pad=0x00] lands at
        // (0x40+o)^3, so logical byte 1 (0x05) is at host 0x40+ (1^3)=0x40+2.
        let logical = |base: usize, o: usize| rdram[(base + o) ^ 3];
        assert_eq!(logical(0x40, 0), 0x00, "port0 type_hi");
        assert_eq!(logical(0x40, 1), 0x05, "port0 type_lo (CONT_TYPE_STANDARD)");
        assert_eq!(logical(0x40, 2), 0x00, "port0 status");
        assert_eq!(logical(0x40, 3), 0x00, "port0 pad");
        // Port 1 absent -> [0,0,0,CONT_NO_RESPONSE_ERROR] swizzled.
        assert_eq!(logical(0x44, 3), CONT_NO_RESPONSE_ERROR, "port1 errno");

        // ctlBitfield: a SINGLE swizzled byte = mask (0x01, only port 0). The
        // flat address 0x80 must stay the 0xEE sentinel (the buggy flat store
        // would overwrite it), and 0x81 must stay 0xEE (the buggy +1 store
        // would clobber this adjacent byte).
        assert_eq!(
            rdram[0x80 ^ 3],
            0x01,
            "bitfield: single swizzled byte, port0 set"
        );
        assert_eq!(
            rdram[0x80], 0xEE,
            "flat bitfield addr untouched (no flat store)"
        );
        assert_eq!(rdram[0x81], 0xEE, "adjacent byte untouched (no +1 store)");
    }
}
