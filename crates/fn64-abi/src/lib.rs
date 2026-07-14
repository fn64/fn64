//! fn64-abi: the extern "C" surface `RecompiledFuncs/*.c` links against.
//!
//! See `docs/DESIGN.md` section 1: this crate is deliberately thin —
//! every symbol here is a signature-and-marshalling adapter over
//! `fn64-runtime`, never a place new policy gets invented. Signatures are
//! transcribed from `aki-recomp/runtime/ABI-SURFACE.md`:
//!
//! - `recomp_context`'s field layout (section (b), from `recomp.h`, MIT):
//!   every `RECOMP_FUNC`/`_recomp` shim takes `(uint8_t* rdram,
//!   recomp_context* ctx)`; `ctx->r4`/`r5`/`r6` are `a0`/`a1`/`a2` per
//!   MIPS o32 calling convention (recomp_context's own gpr field order,
//!   section (b)), which is how generated code passes a `_recomp` shim's
//!   arguments — there is no separate typed C prototype for each shim in
//!   the generated code; every one is called through the same
//!   `(rdram, ctx)` shape and reads its own arguments out of `ctx`.
//! - `pause_self` (section (a), recomp.h dispatch helpers table: NWXE x3,
//!   NW4E x2 call sites) — the yield primitive `docs/DESIGN.md` section 2
//!   models as a stackful-coroutine `yield_now()`.
//! - `osCreateMesgQueue_recomp` / `osSendMesg_recomp` (section (a), the
//!   `_recomp` shim inventory) — the two shims `docs/DESIGN.md` section 2
//!   discusses in the most depth (rung 12's reset invariant; rung 18b's
//!   root cause in the blocking-send path).
//!
//! This module intentionally does NOT yet implement real scheduling —
//! wave 2/3 in `docs/DESIGN.md` section 5 own that. What's here is the
//! correct extern surface shape, wired to `fn64-runtime`'s real
//! `MesgQueue`, with a loud, named panic (per `AGENTS.md`'s "loud traps,
//! no silent shrugs") anywhere real executor integration is still missing,
//! rather than a silent no-op.

use fn64_runtime::{Mesg, MesgQueue, RdramAddr, SendResult};
use std::cell::RefCell;
use std::collections::HashMap;

/// MIPS `recomp_context`, field layout per ABI-SURFACE.md section (b),
/// verbatim struct order from `refs/N64RecompSource/include/recomp.h`
/// (MIT). Only the fields generated code is documented to actually
/// dereference (section (b)'s "fields_actually_touched_by_generated_code")
/// are given real storage; the rest of the real struct (fpr regs, hi/lo,
/// f_odd, status_reg, mips3_float_mode) is out of scope for these 3
/// representative symbols and omitted here rather than faked — a future
/// wave adding a symbol that touches them extends this struct then, with
/// its own citation.
#[repr(C)]
pub struct RecompContext {
    pub r0: u64,
    pub r1: u64,
    pub r2: u64,
    pub r3: u64,
    pub r4: u64,
    pub r5: u64,
    pub r6: u64,
    pub r7: u64,
}

/// Opaque handle stashed in `ctx->r4` (`a0`, the `OSMesgQueue*` argument)
/// by `osCreateMesgQueue_recomp`'s caller and passed back into
/// `osSendMesg_recomp`. Real N64Recomp-generated code passes a raw guest
/// rdram address; this smoke-test-scale implementation resolves it to a
/// `MesgQueue` in a process-global table keyed by that address, rather
/// than attempting a full rdram-backed struct layout for these 3 symbols
/// — a real wave-3 implementation replaces this table with the executor's
/// own queue registry (see `docs/DESIGN.md` section 2's `EventTable`).
struct QueueTable {
    queues: HashMap<u32, MesgQueue>,
}

thread_local! {
    static QUEUES: RefCell<QueueTable> = RefCell::new(QueueTable { queues: HashMap::new() });
}

/// `pause_self` (recomp.h dispatch helper, ABI-SURFACE.md section (a)).
/// The yield primitive `docs/DESIGN.md` section 2 designates as a
/// stackful-coroutine `yield_now()` call. No executor exists yet (wave 2);
/// this is the correctly-shaped, loudly-unimplemented stub `AGENTS.md`
/// requires in place of a silent no-op.
#[no_mangle]
pub extern "C" fn pause_self(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "pause_self: no coroutine executor wired yet (docs/DESIGN.md section 2, wave 2) \
         -- this MUST panic loudly per AGENTS.md rather than silently return, \
         since a silent return here would let a second logical thread's code \
         run without ever actually yielding, which is exactly the invariant \
         section 2 exists to prevent."
    );
}

/// `osCreateMesgQueue_recomp` (ABI-SURFACE.md section (a): NWXE x20 call
/// sites currently named). MIPS signature `osCreateMesgQueue(OSMesgQueue
/// *mq, OSMesg *msg, s32 count)` — `a0`=mq (`ctx->r4`), `a1`=msg
/// (`ctx->r5`), `a2`=count (`ctx->r6`), per o32 calling convention and
/// `recomp_context`'s gpr field order (section (b)).
///
/// Always produces a genuinely empty queue (`fn64_runtime::MesgQueue::new`)
/// -- this is rung 12's load-bearing reset, see `docs/DESIGN.md` section 2
/// and 3: there is no path here that could leave a stale/sentinel value in
/// a blocked list, because `MesgQueue::new` is the only constructor and it
/// always starts empty.
///
/// # Safety
/// `ctx` must be a valid, non-null pointer to a live `RecompContext`, as
/// every `RECOMP_FUNC`/`_recomp` shim's caller (N64Recomp-generated C) is
/// contractually required to pass (ABI-SURFACE.md section (b)/(a)).
#[no_mangle]
pub unsafe extern "C" fn osCreateMesgQueue_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4).offset();
    let count = ctx.r6 as usize;

    QUEUES.with(|t| {
        t.borrow_mut()
            .queues
            .insert(mq_addr, MesgQueue::new(count.max(1)));
    });
}

/// `osSendMesg_recomp` (ABI-SURFACE.md section (a): NWXE x27 call sites
/// currently named). MIPS signature `osSendMesg(OSMesgQueue *mq, OSMesg
/// msg, s32 flag)` — `a0`=mq (`ctx->r4`), `a1`=msg (`ctx->r5`), `a2`=flag
/// (`ctx->r6`, `OS_MESG_BLOCK`/`OS_MESG_NOBLOCK`).
///
/// This is rung 18b's exact root-cause path (`docs/DESIGN.md` section 2):
/// the reference runtime's crash was eventually traced to a genuinely
/// concurrent second host thread's `osSendMesg` blocking-insert on a
/// shared queue struct, invisible to the scheduler's own lock. Here the
/// blocking path is a loud, named panic rather than a silent success,
/// because the "block until space frees" semantics require the wave-2
/// executor (coroutine yield + wake-on-space) that does not exist yet; a
/// non-blocking send that succeeds is implemented for real against
/// `fn64_runtime::MesgQueue`, since it needs no scheduler integration at
/// all.
///
/// # Safety
/// `ctx` must be a valid, non-null pointer to a live `RecompContext` (same
/// contract as `osCreateMesgQueue_recomp` above).
#[no_mangle]
pub unsafe extern "C" fn osSendMesg_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4).offset();
    let msg: Mesg = ctx.r5 as u32;
    const OS_MESG_NOBLOCK: u64 = 0;

    QUEUES.with(|t| {
        let mut table = t.borrow_mut();
        let queue = table.queues.get_mut(&mq_addr).expect(
            "osSendMesg_recomp called on a queue never created via osCreateMesgQueue_recomp",
        );

        match queue.try_send(msg) {
            SendResult::Delivered => {}
            SendResult::WouldBlock if ctx.r6 == OS_MESG_NOBLOCK => {
                // Real osSendMesg semantics: OS_MESG_NOBLOCK on a full
                // queue returns an error code without blocking. Modeling
                // the return-value marshalling is a wave-3 task; dropping
                // the message here would be a silent shrug, so this stays
                // loud until that lands.
                unimplemented!(
                    "osSendMesg_recomp: OS_MESG_NOBLOCK-on-full return-value \
                     marshalling not yet wired (wave 3)"
                );
            }
            SendResult::WouldBlock => {
                unimplemented!(
                    "osSendMesg_recomp: blocking send on a full queue requires \
                     the wave-2 coroutine executor (register on MesgQueue's \
                     blocked_on_send list, yield, wait for a wake) -- this is \
                     rung 18b's exact root-cause path (docs/DESIGN.md section 2), \
                     so it stays a loud panic rather than a silent no-op until \
                     the executor exists to make it actually safe."
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(r4: u64, r5: u64, r6: u64) -> RecompContext {
        RecompContext {
            r0: 0,
            r1: 0,
            r2: 0,
            r3: 0,
            r4,
            r5,
            r6,
            r7: 0,
        }
    }

    #[test]
    fn create_then_nonblocking_send_succeeds() {
        let mq_vram: u64 = 0xFFFF_FFFF_8005_7228; // sign-extended KSEG0 form
        let mut create_ctx = ctx_with(mq_vram, 0, 4);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };

        let mut send_ctx = ctx_with(
            mq_vram, 0xABCD, 0, /* OS_MESG_NOBLOCK, queue not full */
        );
        unsafe { osSendMesg_recomp(std::ptr::null_mut(), &mut send_ctx as *mut _) };

        QUEUES.with(|t| {
            let addr = RdramAddr::from_gpr(mq_vram).offset();
            let q = t.borrow_mut();
            let q = q.queues.get(&addr).unwrap();
            assert_eq!(q.capacity(), 4);
        });
    }

    // `pause_self` is a plain `extern "C" fn` (the real ABI shape generated
    // C calls, matching every other symbol here) -- a Rust panic cannot
    // unwind across that boundary and aborts the process instead (Rust's
    // own defined behavior for an unwind reaching a non-"C-unwind" extern
    // boundary). That abort IS the loud trap `AGENTS.md` requires, so it's
    // verified as a subprocess exit rather than `#[should_panic]`, which
    // requires an in-process catchable unwind and would otherwise abort
    // the whole test harness (confirmed: it did, before this was fixed).
    #[test]
    fn pause_self_is_a_loud_stub_that_aborts_rather_than_silently_no_ops() {
        use std::process::Command;

        let exe = std::env::current_exe().expect("current_exe");
        let status = Command::new(exe)
            .arg("--exact")
            .arg("tests::__pause_self_abort_subprocess_entry")
            .arg("--ignored")
            .arg("--nocapture")
            .env("FN64_ABI_RUN_PAUSE_SELF_ABORT_CHECK", "1")
            .status()
            .expect("failed to spawn subprocess");

        // A SIGABRT/SIGILL-style abort from `unimplemented!` reaching an
        // extern "C" boundary is a signal-terminated exit, never success
        // and never a normal Err-style nonzero -- assert it did NOT exit
        // successfully, which is the observable proof pause_self did not
        // silently return.
        assert!(
            !status.success(),
            "pause_self must abort (loud trap), not return successfully"
        );
    }

    #[test]
    #[ignore] // only ever run directly, by the subprocess harness above
    fn __pause_self_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_PAUSE_SELF_ABORT_CHECK").is_some() {
            pause_self(std::ptr::null_mut(), std::ptr::null_mut());
        }
    }
}
