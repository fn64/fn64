#![cfg(test)]

use super::*;

thread_local! {
    /// ABI-only tests that drive live VI registers without booting a complete
    /// process still need the same exact physical-device authority as
    /// production presentation. Keeping this owned per test thread makes the
    /// raw registration stable for the whole test and avoids an empty-memory
    /// compatibility fallback at the renderer seam.
    static TEST_PRESENT_RDRAM: std::cell::RefCell<Box<[u8]>> =
        std::cell::RefCell::new(vec![0; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE].into_boxed_slice());
}

pub(crate) fn ctx_zeroed() -> RecompContext {
    RecompContext::zeroed()
}

pub(crate) fn ctx_with(r4: u64, r5: u64, r6: u64) -> RecompContext {
    let mut ctx = ctx_zeroed();
    ctx.r4 = r4;
    ctx.r5 = r5;
    ctx.r6 = r6;
    ctx
}

struct CompleteRenderBackend;

impl fn64_render::RenderBackend for CompleteRenderBackend {
    fn create(&mut self, _cfg: &fn64_render::RenderConfig) -> Result<(), fn64_render::RenderError> {
        Ok(())
    }

    fn observe_non_rdp_write16(
        &mut self,
        _write: fn64_render::NonRdpWrite16,
    ) -> fn64_render::NonRdpWrite16Disposition {
        fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar
    }

    fn process_task(
        &mut self,
        _rdram: &mut [u8],
        _rsp_memory: &mut fn64_runtime::RspMemory,
        _task: &fn64_render::OsTask,
        _output_addr: u32,
    ) -> Result<fn64_render::FrameStatus, fn64_render::RenderError> {
        Ok(fn64_render::FrameStatus::Complete)
    }

    fn process_rdp_commands(
        &mut self,
        _rdram: &mut [u8],
        _start: u32,
        _end: u32,
        _output_addr: u32,
        _wait_for_completion: bool,
    ) -> Result<fn64_render::FrameStatus, fn64_render::RenderError> {
        Ok(fn64_render::FrameStatus::Complete)
    }

    fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
        fn64_render::DpFullSyncStatus::Reached
    }

    fn present(
        &mut self,
        _request: fn64_render::PresentRequest<'_>,
    ) -> Result<(), fn64_render::RenderError> {
        Ok(())
    }

    fn resize(&mut self, _w: u32, _h: u32) {}

    fn supported_ucodes(&self) -> &[fn64_render::UcodeId] {
        &[]
    }
}

/// Make a test's renderer dependency explicit without assigning it drawing
/// semantics irrelevant to that test.
pub(crate) fn install_complete_render_backend(rdram_len: usize) {
    install_test_present_rdram();
    set_render_backend(Box::new(CompleteRenderBackend), rdram_len);
}

pub(crate) fn install_test_present_rdram() {
    TEST_PRESENT_RDRAM.with(|storage| {
        let mut storage = storage.borrow_mut();
        with_host(|host| {
            host.runtime_rdram = storage.as_mut_ptr();
            host.runtime_rdram_len = storage.len();
        });
    });
}

/// An rdram buffer for a test that hands guest KSEG0 vram addresses to a
/// shim. `len_from_vram` is the HIGHEST KSEG0 address the test hands to any
/// shim; the buffer is sized to cover that address's rdram offset, so a
/// shim's own `RdramAddr::from_gpr` translation lands inside it.
///
/// Hand-sizing (`vec![0u8; 64]`) is what made the `two_threads_blocked_...`
/// flake: the test passed `msg_out` at KSEG0+0x40 to `osRecvMesg_recomp`,
/// whose faithful `MEM_W` write then stored 4 bytes at rdram offset 0x40 --
/// exactly one word past a 64-byte `Vec`. The 4-byte heap overflow silently
/// smashed whatever `malloc` had placed next; when that happened to be a
/// live `corosensei::DefaultStack`, its `base` field became 0xff807eef and
/// the stack's `Drop` called `munmap(0xff807eef)` -> EINVAL -> `debug_assert`
/// panic inside a thread-local destructor -> "fatal runtime error: thread
/// local panicked on drop" killing the whole test process with SIGTRAP while
/// every test still reported `ok`.
///
/// The exact interleaving this closes (AGENTS.md "name the interleaving"):
/// the write is unconditional, but it only KILLS the process when the
/// allocator has placed a live `DefaultStack` immediately after the 64-byte
/// `Vec`. Sequence: (1) thread A's coroutine stack is mmap'd and its
/// `DefaultStack` control block heap-allocated; (2) `rdram_b`'s `Vec` is
/// allocated into the free chunk adjacent to that block; (3) B's blocked
/// `osRecvMesg_recomp` is woken and stores its message word at rdram offset
/// 0x40 -- 4 bytes past the `Vec`, onto `DefaultStack::base`; (4) at
/// thread-local teardown `Drop` calls `munmap(base - mmap_len)` with the
/// smashed base -> EINVAL -> `debug_assert_eq!` panics inside a TLS
/// destructor -> unwind is forbidden there -> process-wide abort. Whether
/// step (2) lands adjacent to step (1)'s block is pure allocator timing,
/// which is why the suite failed ~15-30% and passed in isolation.
///
/// A buffer derived from the guest address the test itself uses cannot
/// develop that off-by-one, so the class is closed at the allocation site
/// rather than re-audited per test (AGENTS.md "mechanism over patch").
pub(crate) fn rdram_for_vram(highest_vram: u64) -> Vec<u8> {
    let end = fn64_runtime::RdramAddr::from_gpr(highest_vram).offset() as usize;
    // +4 so a full `MEM_W` word AT `highest_vram` is in bounds, not just its
    // first byte.
    vec![0u8; end + 4]
}

pub(crate) fn spawn_test_thread(id: ThreadId, pri: Priority, body: impl FnOnce() + 'static) {
    with_executor(|exec| {
        exec.create_thread(id, pri, move |yielder, first_input| {
            with_active_yielder(id, std::ptr::null_mut(), yielder, || {
                let _ = first_input;
                body();
            });
        });
        exec.start_thread(id);
    });
}

pub(crate) fn run_to_idle_with_yielder_plumbing() {
    run_to_idle();
}

// `get_function`/`pause_self`/`switch_error`/`do_break`/the VI-family loud traps and
// `__osSiRawStartDma_recomp`/`osSpTaskYielded_recomp` are all plain
// `extern "C" fn`s -- a Rust panic cannot unwind across that boundary
// and aborts the process instead, so each is verified as a subprocess
// exit rather than `#[should_panic]`, which requires an in-process
// catchable unwind and would otherwise abort the whole test harness --
// same pattern a prior wave established for `osCreateThread_recomp`/
// `osStartThread_recomp` before this wave wired their real dispatch.
// `!status.success()` alone is too weak twice over, and both weaknesses were
// live defects found by this wave:
//   1. ANY nonzero death counted as "the trap fired". The pi child was dying
//      of SIGBUS (138) on an out-of-bounds read ~2 GiB past its buffer and
//      never reaching its `no ROM installed` panic at all -- a green test
//      proving nothing. Asserting SIGABRT specifically is what caught it.
//   2. A child that matches ZERO tests (a renamed/moved `#[ignore]` entry, or
//      a runner that filters differently) exits 0 -- reported here as "the
//      trap did not fire" rather than "the harness pointed at nothing".
//      Verified: `--exact <nonexistent> --ignored` exits 0.
// So: require the child to have RUN exactly one test, and to have died by
// SIGABRT -- the signal a Rust panic across an `extern "C"` boundary raises.
pub(crate) fn assert_subprocess_aborts(test_name: &str) {
    use std::os::unix::process::ExitStatusExt;

    let exe = std::env::current_exe().expect("current_exe");
    let out = std::process::Command::new(exe)
        .arg("--exact")
        .arg(test_name)
        .arg("--ignored")
        .arg("--nocapture")
        .env("FN64_ABI_RUN_ABORT_CHECK", "1")
        .output()
        .expect("failed to spawn subprocess");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("running 1 test"),
        "{test_name}: subprocess matched no test (harness pointed at nothing -- renamed or \
         un-`#[ignore]`d entry?), so its exit status proves nothing about the trap.\n\
         stdout:\n{stdout}"
    );
    const SIGABRT: i32 = 6;
    assert_eq!(
        out.status.signal(),
        Some(SIGABRT),
        "{test_name} must abort via its loud trap (SIGABRT, i.e. a Rust panic crossing the \
         `extern \"C\"` boundary). Got status {:?}. A SIGSEGV/SIGBUS here means the child hit \
         undefined behavior BEFORE reaching the trap -- the test would still be green while \
         proving nothing.\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}
