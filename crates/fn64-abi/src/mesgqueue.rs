use super::*;

// ---------------------------------------------------------------------
// Message queue: osCreateMesgQueue / osSendMesg / osRecvMesg /
// osSetEventMesg. Unchanged behavior from prior wave, RecompContext
// fields renamed to the full struct (still r4/r5/r6, same o32 slots).
// ---------------------------------------------------------------------

/// `osCreateMesgQueue(OSMesgQueue *mq, OSMesg *msg, s32 count)`.
///
/// # Safety
/// `ctx` must be a valid, non-null pointer to a live `RecompContext`.
#[no_mangle]
pub unsafe extern "C" fn osCreateMesgQueue_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let count = ctx.r6 as usize;
    if debug_boot_enabled() {
        let tid = ACTIVE_THREAD_ID.with(|c| c.get());
        eprintln!(
            "[DEBUG osCreateMesgQueue] thread={tid:?} mq={:#x} count={count}",
            mq_addr.offset()
        );
    }
    with_executor(|exec| exec.create_mesg_queue(mq_addr, count.max(1)));
}

/// `osSendMesg(OSMesgQueue *mq, OSMesg msg, s32 flag)`.
///
/// # Safety
/// Same contract as `osCreateMesgQueue_recomp`.
#[no_mangle]
pub unsafe extern "C" fn osSendMesg_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    crate::probe_sp_region("send", ctx);
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let msg: Mesg = ctx.r5 as u32;
    let may_block = ctx.r6 == OS_MESG_BLOCK;
    let debug_send = fn64_runtime::debug_send_diagnostics();
    if debug_send.enabled() {
        let tid = ACTIVE_THREAD_ID.with(|c| c.get());
        eprintln!(
            "[DEBUG osSendMesg_recomp] active_thread={tid:?} mq_offset={:#x} msg={msg:#x} \
             may_block={may_block} r29(sp)={:#x}",
            mq_addr.offset(),
            ctx.r29
        );
        if let Some(count) = debug_send.message_words() {
            let offset = RdramAddr::from_gpr(u64::from(msg)).offset() as usize;
            let byte_len = count
                .checked_mul(4)
                .expect("FN64_DEBUG_SEND_WORDS byte length overflow");
            let end = offset
                .checked_add(byte_len)
                .expect("FN64_DEBUG_SEND_WORDS range overflow");
            let allocation_len = with_host(|host| host.runtime_rdram_len);
            if end <= allocation_len {
                let bytes = unsafe { std::slice::from_raw_parts(rdram.add(offset), byte_len) };
                let words: Vec<_> = bytes
                    .chunks_exact(4)
                    .map(|bytes| u32::from_ne_bytes(bytes.try_into().expect("four message bytes")))
                    .collect();
                eprintln!("[DEBUG osSendMesg_recomp] msg_words={words:08x?}");
            }
        }
    }

    let sent = match suspend_active_coroutine(Yield::BlockOnSend {
        mq_addr,
        msg,
        may_block,
        jam: false,
    }) {
        Resume::SendUnblocked => true,
        Resume::WouldBlock => false,
        other => panic!(
            "osSendMesg_recomp: resumed from a BlockOnSend yield with an unexpected Resume \
             variant {other:?}"
        ),
    };
    // Return value in $v0: 0 on enqueue, -1 when a NOBLOCK send found the
    // queue full (public libultra Function Reference, Message Manager `osSendMesg`,
    // "Explanation"; see `docs/DESIGN.md` § "OSMesgQueue semantics" for provenance).
    // Symmetric with the recv below -- previously never written, same stale-$v0 bug.
    ctx.r2 = if sent { 0 } else { -1i64 as u64 };
}

/// `OS_MESG_NOBLOCK`/`OS_MESG_BLOCK`, per the public libultra manual.
#[allow(dead_code)]
const OS_MESG_NOBLOCK: u64 = 0;
const OS_MESG_BLOCK: u64 = 1;

/// `osRecvMesg(OSMesgQueue *mq, OSMesg *msg, s32 flag)`.
///
/// # Safety
/// `rdram`/`ctx` must be valid per the same contract as every other shim.
#[no_mangle]
pub unsafe extern "C" fn osRecvMesg_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    crate::probe_sp_region("recv", ctx);
    if crate::boot_probe_enabled() && (ctx.r4 as u32) < 0x8000_0400 {
        eprintln!(
            "[boot-probe] osRecvMesg NON-POINTER mq={:#x} thread={:#x} restored_slot={:#010x}",
            ctx.r4 as u32,
            crate::current_thread_id("probe"),
            (ctx.r29 as u32).wrapping_sub(8),
        );
    }
    // Correction (2026-07-14): check the RAW register for null, not the
    // translated `RdramAddr`. A real `msg == NULL` call (public libultra
    // manual's documented "pass NULL to just wait, discarding the message"
    // form -- OoT's own boot path uses exactly this: `DmaMgr_DmaRomToRam`'s
    // real call site, `games/OOTU/RecompiledFuncs/funcs_0.c:975-980`, is
    // `osRecvMesg(s6, a1=0, 1)`) has `ctx.r5 == 0`, but
    // `RdramAddr::from_gpr(0)` computes `0u64.wrapping_sub(0xFFFFFFFF80000000)`,
    // which is `0x8000_0000`, NOT `0` -- the OLD `msg_out_addr.offset() !=
    // 0` guard below never actually caught a real null pointer, so this
    // shim wrote the delivered message to rdram OFFSET 0x8000_0000 (a real
    // out-of-bounds write for any buffer smaller than that, and a silent
    // wrong-address write even for large enough buffers) every time a real
    // ROM legitimately passed NULL. First caught by `examples/oot-boot`'s
    // real boot run (`osRecvMesg_recomp` SIGSEGV at rdram offset
    // 0x8000_0000-ish inside `DmaMgr_Init`, thread 1's very first blocking
    // receive). Fixed by testing the UNTRANSLATED register value for zero.
    let msg_out_is_null = ctx.r5 == 0;
    let msg_out_addr = RdramAddr::from_gpr(ctx.r5);
    let may_block = ctx.r6 == OS_MESG_BLOCK;

    let delivered = match suspend_active_coroutine(Yield::BlockOnRecv { mq_addr, may_block }) {
        Resume::Delivered(msg) => Some(msg),
        Resume::WouldBlock => None,
        other => panic!(
            "osRecvMesg_recomp: resumed from a BlockOnRecv yield with an unexpected Resume \
             variant {other:?}"
        ),
    };

    if let Some(msg) = delivered {
        if !msg_out_is_null {
            let o = msg_out_addr.offset() as usize;
            // Native byte order, matching MEM_W's real semantics -- see
            // `read_stack_word`'s doc comment for the full correction this
            // wave made (a prior wave's big-endian assumption was wrong).
            unsafe {
                std::ptr::copy_nonoverlapping((msg as i32).to_ne_bytes().as_ptr(), rdram.add(o), 4);
            }
        }
    }

    // Return value in $v0 (`ctx.r2`): 0 on delivery, -1 when a NOBLOCK recv
    // found the queue empty (public libultra Function Reference, Message Manager
    // `osRecvMesg`, "Explanation"; see `docs/DESIGN.md` § "OSMesgQueue semantics").
    // NOBLOCK drain loops (e.g. OoT's `Sched_HandleNotification`, asm
    // 0x800A3180 `beq $v0, -1`) test exactly this to detect an empty queue and
    // stop. Leaving $v0 stale (the prior omission -- `ctx` was borrowed `&*`,
    // this write was simply never made) makes that loop never see -1, so the
    // Scheduler thread spins a NOBLOCK poll forever and virtual time never
    // advances (examples/oot-boot: sim_time stuck at 0, run_one_step always
    // returns true). Written last, after all field reads.
    ctx.r2 = if delivered.is_some() { 0 } else { -1i64 as u64 };
}

/// `osSetEventMesg(OSEvent event, OSMesgQueue *mq, OSMesg msg)`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSetEventMesg_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    if crate::boot_probe_enabled() {
        eprintln!(
            "[boot-probe] osSetEventMesg(event={}, mq={:#010x}, msg={:#010x})",
            ctx.r4 as u32, ctx.r5 as u32, ctx.r6 as u32
        );
    }
    let event = ctx.r4 as u32;
    let mq_addr = RdramAddr::from_gpr(ctx.r5);
    let msg: Mesg = ctx.r6 as u32;
    if debug_boot_enabled() {
        let tid = ACTIVE_THREAD_ID.with(|c| c.get());
        eprintln!(
            "[DEBUG osSetEventMesg] thread={tid:?} event={event} mq={:#x} msg={msg:#x}",
            mq_addr.offset()
        );
    }
    with_executor(|exec| exec.set_event_mesg(event, mq_addr, msg));
}

/// `osJamMesg(OSMesgQueue *mq, OSMesg msg, s32 flag) -> s32` -- priority-jump
/// variant of `osSendMesg`: front-inserts the message (jammesg.c:16-17
/// `first = (first + msgCount - 1) % msgCount; msg[first] = msg`) so it is the
/// next one received, and returns 0 on enqueue / -1 on a NOBLOCK full queue
/// (jammesg.c:23 / :12). Reachable via audio DMA: `osEPiStartDma`
/// (epidma.c:18-20) calls `osJamMesg` when `mb->hdr.pri == 1`, and
/// `AudioLoad_Dma` FastCopy loads pass `OS_MESG_PRI_HIGH == 1` (load.c:1047,
/// 1055; pi.h:76). o32: all three args are 32-bit -> mq=$a0=r4, msg=$a1=r5,
/// flag=$a2=r6, return $v0=r2 -- mirrors `osSendMesg_recomp` exactly, only
/// the insertion end differs (`jam: true`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osJamMesg_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let msg: Mesg = ctx.r5 as u32;
    let may_block = ctx.r6 == OS_MESG_BLOCK;

    let jammed = match suspend_active_coroutine(Yield::BlockOnSend {
        mq_addr,
        msg,
        may_block,
        jam: true,
    }) {
        Resume::SendUnblocked => true,
        Resume::WouldBlock => false,
        other => panic!(
            "osJamMesg_recomp: resumed from a BlockOnSend yield with an unexpected Resume \
             variant {other:?}"
        ),
    };
    // $v0: 0 on front-insert, -1 when a NOBLOCK jam found the queue full.
    ctx.r2 = if jammed { 0 } else { -1i64 as u64 };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use fn64_runtime::{RecvMesgOutcome, SendMesgOutcome};

    #[test]
    fn create_then_nonblocking_send_succeeds() {
        let mq_vram: u64 = 0xFFFF_FFFF_8005_7228;
        let mut create_ctx = ctx_with(mq_vram, 0, 4);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };

        spawn_test_thread(101, 1, move || {
            let mut send_ctx = ctx_with(mq_vram, 0xABCD, OS_MESG_NOBLOCK);
            unsafe { osSendMesg_recomp(std::ptr::null_mut(), &mut send_ctx as *mut _) };
        });
        run_to_idle_with_yielder_plumbing();

        with_executor(|exec| {
            let addr = RdramAddr::from_gpr(mq_vram);
            assert_eq!(exec.queue_capacity(addr), 4);
        });
    }

    #[test]
    fn blocking_send_on_full_queue_actually_yields_and_is_woken() {
        let mq_vram: u64 = 0xFFFF_FFFF_8006_0000;
        let mq_addr = RdramAddr::from_gpr(mq_vram);
        let mut create_ctx = ctx_with(mq_vram, 0, 1);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };

        with_executor(|exec| {
            let outcome = exec.send_mesg(0, mq_addr, 1, false);
            assert_eq!(outcome, SendMesgOutcome::Delivered);
        });

        let delivered_second = std::rc::Rc::new(std::cell::RefCell::new(false));
        let delivered_second2 = delivered_second.clone();
        spawn_test_thread(102, 1, move || {
            let mut send_ctx = ctx_with(mq_vram, 2, OS_MESG_BLOCK);
            unsafe { osSendMesg_recomp(std::ptr::null_mut(), &mut send_ctx as *mut _) };
            *delivered_second2.borrow_mut() = true;
        });

        run_one_step();
        assert!(!*delivered_second.borrow());

        with_executor(|exec| {
            let outcome = exec.recv_mesg(999, mq_addr, false);
            assert_eq!(outcome, RecvMesgOutcome::Delivered(1));
        });
        run_to_idle_with_yielder_plumbing();
        assert!(*delivered_second.borrow());
    }

    #[test]
    fn blocking_recv_on_empty_queue_yields_and_receives_real_message() {
        let mq_vram: u64 = 0xFFFF_FFFF_8007_0000;
        let mq_addr = RdramAddr::from_gpr(mq_vram);
        let mut create_ctx = ctx_with(mq_vram, 0, 1);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };

        let mut rdram = vec![0u8; 64];
        let rdram_ptr = rdram.as_mut_ptr();
        let msg_out_vram: u64 = 0xFFFF_FFFF_8000_0020;

        spawn_test_thread(103, 1, move || {
            let mut recv_ctx = ctx_with(mq_vram, msg_out_vram, OS_MESG_BLOCK);
            unsafe { osRecvMesg_recomp(rdram_ptr, &mut recv_ctx as *mut _) };
        });
        run_one_step();

        with_executor(|exec| {
            let outcome = exec.send_mesg(0, mq_addr, 0x1234_5678, false);
            assert_eq!(outcome, SendMesgOutcome::Delivered);
        });
        run_to_idle_with_yielder_plumbing();

        // Native byte order -- see read_stack_word's doc comment for why
        // MEM_W is a native-endian word access, not big-endian.
        let written = i32::from_ne_bytes(rdram[0x20..0x24].try_into().unwrap());
        assert_eq!(written, 0x1234_5678);
    }

    /// Regression test for the real infinite-spin the OoT boot harness hit
    /// (2026-07-14): `osRecvMesg_recomp` never wrote `ctx.r2` ($v0), so a
    /// NOBLOCK-recv drain loop that tests `$v0 == -1` to detect an empty
    /// queue never saw -1 and spun forever. The concrete caller was OoT's
    /// Scheduler thread (`Sched_HandleNotification`, asm 0x800A3174 does a
    /// NOBLOCK `osRecvMesg` then 0x800A3180 `beq $v0, -1` to exit) -- the
    /// spin pinned `run_one_step` always-runnable, so virtual time never
    /// advanced (`examples/oot-boot`: sim_time stuck at 0). The public libultra
    /// Function Reference, Message Manager `osRecvMesg`, "Explanation",
    /// specifies that a NOBLOCK receive on an empty queue returns -1.
    ///
    /// Seed `ctx.r2` with a realistic STALE non-`-1` value first, so a
    /// regression that stops writing `$v0` fails here even though a
    /// zeroed-`ctx` setup would have masked it (0 != -1 by luck, but the
    /// delivered-path test below pins the 0 case too).
    #[test]
    fn noblock_recv_on_empty_queue_returns_minus_one_in_v0_even_with_stale_r2() {
        let mq_vram: u64 = 0xFFFF_FFFF_800B_0000;
        let mut create_ctx = ctx_with(mq_vram, 0, 1);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };

        let observed = std::rc::Rc::new(std::cell::RefCell::new(None));
        let observed2 = observed.clone();
        spawn_test_thread(110, 5, move || {
            let mut recv_ctx = ctx_with(mq_vram, 0, OS_MESG_NOBLOCK);
            recv_ctx.r2 = 0x1234_5678; // stale non-(-1), as a real caller's $v0 would hold
            unsafe { osRecvMesg_recomp(std::ptr::null_mut(), &mut recv_ctx as *mut _) };
            *observed2.borrow_mut() = Some(recv_ctx.r2);
        });
        // A NOBLOCK recv on an empty queue never parks: the thread runs to
        // completion in a single step (this is exactly the yield that made
        // the Sched thread always-runnable before the fix).
        run_to_idle_with_yielder_plumbing();

        assert_eq!(
            observed.borrow().expect("recv thread ran"),
            -1i64 as u64,
            "NOBLOCK osRecvMesg on an empty queue must return -1 in $v0, or a \
             `beq $v0, -1` drain loop (OoT's Sched thread) spins forever"
        );
    }

    /// The delivered path must pin `$v0 = 0` (success), again seeding a
    /// stale non-zero `$v0` so a regression that only writes -1 (or nothing)
    /// on the empty path but leaves the success path stale is still caught.
    #[test]
    fn recv_that_delivers_a_message_returns_zero_in_v0_even_with_stale_r2() {
        let mq_vram: u64 = 0xFFFF_FFFF_800C_0000;
        let mq_addr = RdramAddr::from_gpr(mq_vram);
        let mut create_ctx = ctx_with(mq_vram, 0, 1);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };
        // Pre-seed one message so the NOBLOCK recv delivers immediately.
        with_executor(|exec| {
            assert_eq!(
                exec.send_mesg(0, mq_addr, 0x0BAD_F00D, false),
                SendMesgOutcome::Delivered
            );
        });

        let observed = std::rc::Rc::new(std::cell::RefCell::new(None));
        let observed2 = observed.clone();
        spawn_test_thread(111, 5, move || {
            let mut recv_ctx = ctx_with(mq_vram, 0, OS_MESG_NOBLOCK);
            recv_ctx.r2 = 0x7777_7777; // stale non-zero
            unsafe { osRecvMesg_recomp(std::ptr::null_mut(), &mut recv_ctx as *mut _) };
            *observed2.borrow_mut() = Some(recv_ctx.r2);
        });
        run_to_idle_with_yielder_plumbing();

        assert_eq!(
            observed.borrow().expect("recv thread ran"),
            0,
            "osRecvMesg that delivers a message must return 0 in $v0"
        );
    }

    /// Symmetric return-value pin for `osSendMesg_recomp`: a NOBLOCK send on
    /// a full queue must return -1 in $v0 (libultra `return sent ? 0 : -1`),
    /// and a send that enqueues returns 0. Same stale-$v0 seeding discipline.
    #[test]
    fn noblock_send_return_value_in_v0_is_minus_one_on_full_zero_on_enqueue() {
        let mq_vram: u64 = 0xFFFF_FFFF_800D_0000;
        let mq_addr = RdramAddr::from_gpr(mq_vram);
        // Capacity-1 queue, pre-filled so the next NOBLOCK send finds it full.
        let mut create_ctx = ctx_with(mq_vram, 0, 1);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };
        with_executor(|exec| {
            assert_eq!(
                exec.send_mesg(0, mq_addr, 0xFEED, false),
                SendMesgOutcome::Delivered
            );
        });

        let full = std::rc::Rc::new(std::cell::RefCell::new(None));
        let full2 = full.clone();
        spawn_test_thread(112, 5, move || {
            let mut send_ctx = ctx_with(mq_vram, 0x1111, OS_MESG_NOBLOCK);
            send_ctx.r2 = 0x2222_2222; // stale non-(-1)
            unsafe { osSendMesg_recomp(std::ptr::null_mut(), &mut send_ctx as *mut _) };
            *full2.borrow_mut() = Some(send_ctx.r2);
        });
        run_to_idle_with_yielder_plumbing();
        assert_eq!(
            full.borrow().expect("send thread ran"),
            -1i64 as u64,
            "NOBLOCK osSendMesg on a full queue must return -1 in $v0"
        );

        // Drain the pre-filled message, then a NOBLOCK send into the now-open
        // slot must return 0.
        with_executor(|exec| {
            assert_eq!(
                exec.recv_mesg(999, mq_addr, false),
                RecvMesgOutcome::Delivered(0xFEED)
            );
        });
        let ok = std::rc::Rc::new(std::cell::RefCell::new(None));
        let ok2 = ok.clone();
        spawn_test_thread(113, 5, move || {
            let mut send_ctx = ctx_with(mq_vram, 0x3333, OS_MESG_NOBLOCK);
            send_ctx.r2 = 0x4444_4444; // stale non-zero
            unsafe { osSendMesg_recomp(std::ptr::null_mut(), &mut send_ctx as *mut _) };
            *ok2.borrow_mut() = Some(send_ctx.r2);
        });
        run_to_idle_with_yielder_plumbing();
        assert_eq!(
            ok.borrow().expect("send thread ran"),
            0,
            "NOBLOCK osSendMesg that enqueues must return 0 in $v0"
        );
    }

    /// Regression test for the coroutine-context-corruption bug the OoT
    /// boot harness hit (Main-resume SIGBUS, PC=0x1 at its 5th `Woken`
    /// resume, immediately after another thread blocked -- see
    /// `crates/fn64-diff/docs/2026-07-14-first-divergence-report.md`).
    ///
    /// `with_active_yielder` (this file) installed `ACTIVE_YIELDER`/
    /// `ACTIVE_THREAD_ID`/`ACTIVE_RDRAM` exactly ONCE per thread, wrapping
    /// the thread's ENTIRE body closure -- correct for the coroutine's own
    /// suspend calls, but never re-armed on each resume. Because every
    /// `GameThread` coroutine runs on the SAME native OS thread and shares
    /// these `thread_local!` cells, once thread A suspends (its body's
    /// `with_active_yielder` call is paused mid-flight, its restore-to-
    /// `previous` code never runs until the body truly returns), the cells
    /// are left pointing at thread A's `Yielder`/id/rdram. If thread B is
    /// then resumed and calls a `_recomp` shim that suspends, it read
    /// thread A's stale `ACTIVE_YIELDER` and called `Yielder::suspend` on
    /// the WRONG coroutine's handle -- corrupting that coroutine's native
    /// resume context. This reproduces the exact two-thread interleaved
    /// shape: thread A blocks first (on a full send queue), then thread B
    /// blocks (on an empty recv queue) while A is still parked -- mirroring
    /// "thread 18 blocks, then thread 3's next resume lands at PC=0x1."
    /// Both are then woken and MUST each observe their own identity/rdram,
    /// not the other's.
    #[test]
    fn two_threads_blocked_and_woken_interleaved_each_resume_their_own_context() {
        // A full queue (capacity 1, already holds one message) so thread A
        // blocks on send; a separate empty queue so thread B blocks on recv.
        let send_mq_vram: u64 = 0xFFFF_FFFF_8009_0000;
        let send_mq_addr = RdramAddr::from_gpr(send_mq_vram);
        let mut create_send_q = ctx_with(send_mq_vram, 0, 1);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_send_q as *mut _) };
        with_executor(|exec| {
            assert_eq!(
                exec.send_mesg(0, send_mq_addr, 0xFFFF_FFFF, false),
                SendMesgOutcome::Delivered
            );
        });

        let recv_mq_vram: u64 = 0xFFFF_FFFF_800A_0000;
        let mut create_recv_q = ctx_with(recv_mq_vram, 0, 1);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_recv_q as *mut _) };

        // Two SEPARATE rdram buffers, each tagged with a value only that
        // thread's own body should ever be able to observe/write, so any
        // cross-wire between the two coroutines' contexts is directly
        // visible rather than needing a crash to notice.
        // Sized from the guest addresses these buffers are actually indexed
        // by, not hand-picked: thread B's `msg_out_vram` (KSEG0+0x40, below)
        // makes `osRecvMesg_recomp` store a word AT rdram offset 0x40, which
        // a `vec![0u8; 64]` ends one word short of -- see `rdram_for_vram`.
        let mut rdram_a = rdram_for_vram(0xFFFF_FFFF_8000_0040);
        let rdram_a_ptr = rdram_a.as_mut_ptr();
        let mut rdram_b = rdram_for_vram(0xFFFF_FFFF_8000_0040);
        let rdram_b_ptr = rdram_b.as_mut_ptr();

        let observed_a = std::rc::Rc::new(std::cell::RefCell::new(None));
        let observed_a2 = observed_a.clone();
        let observed_b = std::rc::Rc::new(std::cell::RefCell::new(None));
        let observed_b2 = observed_b.clone();

        const THREAD_A: ThreadId = 500;
        const THREAD_B: ThreadId = 501;

        // Thread A: blocks on osSendMesg (queue already full). Once
        // unblocked, records its OWN thread id (via osGetThreadId_recomp,
        // which reads ACTIVE_THREAD_ID) and writes a marker into ITS OWN
        // rdram buffer at a fixed offset.
        with_executor(|exec| {
            exec.create_thread(THREAD_A, 5, move |yielder, first_input| {
                with_active_yielder(THREAD_A, rdram_a_ptr, yielder, || {
                    let _ = first_input;
                    let mut send_ctx = ctx_with(send_mq_vram, 0xAAAA, OS_MESG_BLOCK);
                    unsafe { osSendMesg_recomp(rdram_a_ptr, &mut send_ctx as *mut _) };
                    let mut id_ctx = ctx_with(0, 0, 0);
                    unsafe { osGetThreadId_recomp(rdram_a_ptr, &mut id_ctx as *mut _) };
                    *observed_a2.borrow_mut() = Some(id_ctx.r2);
                    unsafe {
                        std::ptr::write(rdram_a_ptr.add(0x30) as *mut u32, 0xA0A0_A0A0u32);
                    }
                });
            });
            exec.start_thread(THREAD_A);
        });

        // Thread B: blocks on osRecvMesg (queue empty). Same shape, its own
        // buffer/marker/expected id.
        with_executor(|exec| {
            exec.create_thread(THREAD_B, 5, move |yielder, first_input| {
                with_active_yielder(THREAD_B, rdram_b_ptr, yielder, || {
                    let _ = first_input;
                    let msg_out_vram: u64 = 0xFFFF_FFFF_8000_0040;
                    let mut recv_ctx = ctx_with(recv_mq_vram, msg_out_vram, OS_MESG_BLOCK);
                    unsafe { osRecvMesg_recomp(rdram_b_ptr, &mut recv_ctx as *mut _) };
                    let mut id_ctx = ctx_with(0, 0, 0);
                    unsafe { osGetThreadId_recomp(rdram_b_ptr, &mut id_ctx as *mut _) };
                    *observed_b2.borrow_mut() = Some(id_ctx.r2);
                    unsafe {
                        std::ptr::write(rdram_b_ptr.add(0x30) as *mut u32, 0xB0B0_B0B0u32);
                    }
                });
            });
            exec.start_thread(THREAD_B);
        });

        // Run both threads until they've each hit their blocking yield --
        // thread A blocks first, THEN thread B blocks while A is still
        // parked (the exact "thread 18 blocks while thread 3 is blocked,
        // then thread 3 resumes" interleaving from the divergence report).
        assert!(run_one_step()); // thread A runs, blocks on send
        assert!(run_one_step()); // thread B runs, blocks on recv
        with_executor(|exec| {
            assert!(!exec.is_thread_dead(THREAD_A));
            assert!(!exec.is_thread_dead(THREAD_B));
        });

        // Wake thread A FIRST (drain a slot on its send queue) while
        // ACTIVE_YIELDER/ACTIVE_THREAD_ID are still stale from thread B --
        // B was the last thread whose body actually ran `with_active_
        // yielder`'s install (it started and blocked AFTER A did), so if
        // the install is never re-armed on resume, A's post-wake
        // `osGetThreadId_recomp` call will incorrectly read B's id. This is
        // the precise "thread 18 blocks [here: B], then thread 3 [here: A]
        // resumes into the wrong saved context" interleaving from the
        // divergence report.
        with_executor(|exec| {
            let outcome = exec.recv_mesg(999, send_mq_addr, false);
            assert_eq!(outcome, RecvMesgOutcome::Delivered(0xFFFF_FFFF));
        });
        assert!(run_one_step()); // resumes A only
                                 // Now wake and resume B.
        with_executor(|exec| {
            let recv_addr = RdramAddr::from_gpr(recv_mq_vram);
            let outcome = exec.send_mesg(0, recv_addr, 0xBEEF, false);
            assert_eq!(outcome, SendMesgOutcome::Delivered);
        });
        run_to_idle();

        with_executor(|exec| {
            assert!(exec.is_thread_dead(THREAD_A));
            assert!(exec.is_thread_dead(THREAD_B));
        });

        // The actual assertion: each thread must have resumed into ITS OWN
        // saved context -- its own thread id via ACTIVE_THREAD_ID, and its
        // own rdram buffer's marker, never the other thread's.
        assert_eq!(
            *observed_a.borrow(),
            Some(THREAD_A as u64),
            "thread A resumed with a stale/wrong ACTIVE_THREAD_ID -- classic sign of \
             ACTIVE_YIELDER/ACTIVE_THREAD_ID left pointing at whichever thread suspended \
             most recently instead of the thread actually being resumed"
        );
        assert_eq!(
            *observed_b.borrow(),
            Some(THREAD_B as u64),
            "thread B resumed with a stale/wrong ACTIVE_THREAD_ID"
        );
        let marker_a = unsafe { std::ptr::read(rdram_a_ptr.add(0x30) as *const u32) };
        let marker_b = unsafe { std::ptr::read(rdram_b_ptr.add(0x30) as *const u32) };
        assert_eq!(
            marker_a, 0xA0A0_A0A0,
            "thread A must write into its OWN rdram buffer, not thread B's"
        );
        assert_eq!(
            marker_b, 0xB0B0_B0B0,
            "thread B must write into its OWN rdram buffer, not thread A's"
        );
        // Cross-check the buffers weren't swapped/aliased: A's buffer must
        // NOT carry B's marker and vice versa.
        let marker_a_has_b =
            unsafe { std::ptr::read(rdram_a_ptr.add(0x30) as *const u32) } == 0xB0B0_B0B0u32;
        let marker_b_has_a =
            unsafe { std::ptr::read(rdram_b_ptr.add(0x30) as *const u32) } == 0xA0A0_A0A0u32;
        assert!(!marker_a_has_b && !marker_b_has_a);
    }

    /// End-to-end pin of the WM2000 (NWXE) grant-crossover fix: when TWO
    /// threads block in `osRecvMesg` on the SAME queue, a send must wake the
    /// HIGHER-priority waiter, even though the lower-priority one parked
    /// first. This is libultra's real semantics (`__osEnqueueThread` keeps
    /// `mq->mtqueue` priority-sorted; `osSendMesg` pops the head), and
    /// WM2000's boot depends on it: gfx runner (thread 17, pri 0x64) and
    /// audio runner (thread 18, pri 0x6E) share one OS_EVENT_SP done queue
    /// (rdram 0x52320), and the audio runner's osSpTaskYield handshake
    /// (funcs_0.c `func_80001024`) assumes every SP-done arriving while both
    /// are parked goes to IT first. Arrival-FIFO handed the audio task's
    /// done to the longer-parked gfx runner -- the crossover that froze boot
    /// at 3 gfx frames / 9307 audio tasks.
    #[test]
    fn send_wakes_highest_priority_receiver_not_first_arrived() {
        let mq_vram: u64 = 0xFFFF_FFFF_800E_0000;
        let mq_addr = RdramAddr::from_gpr(mq_vram);
        let mut create_ctx = ctx_with(mq_vram, 0, 8);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };

        let got_gfx = std::rc::Rc::new(std::cell::RefCell::new(None));
        let got_gfx2 = got_gfx.clone();
        let got_audio = std::rc::Rc::new(std::cell::RefCell::new(None));
        let got_audio2 = got_audio.clone();

        // Mirror the deadlock's park order and priorities: the LOWER-pri
        // "gfx runner" parks FIRST, the HIGHER-pri "audio runner" second.
        spawn_test_thread(17, 0x64, move || {
            let mut recv_ctx = ctx_with(mq_vram, 0, OS_MESG_BLOCK);
            unsafe { osRecvMesg_recomp(std::ptr::null_mut(), &mut recv_ctx as *mut _) };
            *got_gfx2.borrow_mut() = Some(());
        });
        run_one_step(); // 17 parks
        spawn_test_thread(18, 0x6E, move || {
            let mut recv_ctx = ctx_with(mq_vram, 0, OS_MESG_BLOCK);
            unsafe { osRecvMesg_recomp(std::ptr::null_mut(), &mut recv_ctx as *mut _) };
            *got_audio2.borrow_mut() = Some(());
        });
        run_one_step(); // 18 parks (later, but higher priority)

        // One message: the "SP done". It must wake thread 18, not 17.
        with_executor(|exec| {
            assert_eq!(
                exec.send_mesg(999, mq_addr, 0x29B, false),
                SendMesgOutcome::Delivered
            );
        });
        run_to_idle_with_yielder_plumbing();

        assert!(
            got_audio.borrow().is_some(),
            "the higher-priority waiter (audio runner) must receive the SP-done \
             even though it parked after the gfx runner"
        );
        assert!(
            got_gfx.borrow().is_none(),
            "the lower-priority, earlier-parked waiter must still be blocked -- \
             waking it instead is exactly the WM2000 grant crossover"
        );

        // Release the still-parked thread 17 so the test tears down with no
        // suspended coroutine (dropping one aborts the test process).
        with_executor(|exec| {
            assert_eq!(
                exec.send_mesg(999, mq_addr, 0x29B, false),
                SendMesgOutcome::Delivered
            );
        });
        run_to_idle_with_yielder_plumbing();
        assert!(got_gfx.borrow().is_some());
    }

    /// osJamMesg front-inserts (jammesg.c) and returns 0 on enqueue -- it must
    /// NOT panic (the old unimplemented!()), and a subsequent recv must see
    /// the jammed message ahead of an already-queued one. Fails against the
    /// bug (unimplemented!() aborts before any assert).
    #[test]
    fn os_jam_mesg_front_inserts_and_returns_zero() {
        let mq_vram: u64 = 0xFFFF_FFFF_8006_A000;
        let mq_addr = RdramAddr::from_gpr(mq_vram);
        let mut create_ctx = ctx_with(mq_vram, 0, 4);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };

        // Pre-queue a normal message, then jam a high-priority one.
        spawn_test_thread(120, 1, move || {
            let mut send_ctx = ctx_with(mq_vram, 0x1111, OS_MESG_NOBLOCK);
            unsafe { osSendMesg_recomp(std::ptr::null_mut(), &mut send_ctx as *mut _) };
            let mut jam_ctx = ctx_with(mq_vram, 0x9999, OS_MESG_NOBLOCK);
            unsafe { osJamMesg_recomp(std::ptr::null_mut(), &mut jam_ctx as *mut _) };
            assert_eq!(jam_ctx.r2, 0, "osJamMesg returns 0 on front-insert");
        });
        run_to_idle_with_yielder_plumbing();

        // The jammed 0x9999 must come out BEFORE the earlier 0x1111.
        with_executor(|exec| {
            assert_eq!(
                exec.recv_mesg(0, mq_addr, false),
                RecvMesgOutcome::Delivered(0x9999)
            );
            assert_eq!(
                exec.recv_mesg(0, mq_addr, false),
                RecvMesgOutcome::Delivered(0x1111)
            );
        });
    }
}
