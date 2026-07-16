#![cfg(test)]

use super::*;

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
pub(crate) fn assert_subprocess_aborts(test_name: &str) {
    let exe = std::env::current_exe().expect("current_exe");
    let status = std::process::Command::new(exe)
        .arg("--exact")
        .arg(test_name)
        .arg("--ignored")
        .arg("--nocapture")
        .env("FN64_ABI_RUN_ABORT_CHECK", "1")
        .status()
        .expect("failed to spawn subprocess");
    assert!(
        !status.success(),
        "{test_name} must abort (loud trap), not return successfully"
    );
}
