//! Process-level regression for terminal teardown of an extern-C guest stack.

use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const CHILD_ENV: &str = "FN64_PROCESS_EXIT_CHILD";
const BLOCKED_CHILD: &str = "blocked-coroutine";
const RENDER_CHILD: &str = "render-continuation";
const QUEUE_VRAM: u64 = 0xffff_ffff_8000_1000;
const MESSAGE_BUFFER_VRAM: u64 = 0xffff_ffff_8000_1100;

unsafe extern "C" fn block_inside_os_recv_mesg(
    rdram: *mut u8,
    _entry_context: *mut fn64_abi::RecompContext,
) {
    let mut create = fn64_abi::RecompContext::zeroed();
    create.r4 = QUEUE_VRAM;
    create.r5 = MESSAGE_BUFFER_VRAM;
    create.r6 = 1;
    unsafe { fn64_abi::osCreateMesgQueue_recomp(rdram, &mut create) };

    let mut recv = fn64_abi::RecompContext::zeroed();
    recv.r4 = QUEUE_VRAM;
    recv.r5 = 0;
    recv.r6 = 1; // OS_MESG_BLOCK
    unsafe { fn64_abi::osRecvMesg_recomp(rdram, &mut recv) };
    panic!("blocked osRecvMesg unexpectedly returned without a message");
}

fn run_blocked_coroutine_child() {
    fn64_abi::load_rom(Vec::new());
    let mut rdram = vec![0u8; 0x2000];
    unsafe {
        fn64_abi::boot_thread0(
            rdram.as_mut_ptr(),
            rdram.len(),
            block_inside_os_recv_mesg,
            0,
            10,
        )
    };

    assert!(fn64_abi::run_one_step()); // deterministic OS-call cycle charge
    assert!(fn64_abi::run_one_step()); // BlockOnRecv
    assert_eq!(fn64_abi::next_runnable_priority(), None);

    let summary = fn64_abi::prepare_process_exit();
    assert_eq!(summary.threads, 1);
    assert_eq!(summary.detached_coroutines, 1);
    assert!(std::panic::catch_unwind(fn64_abi::sim_time).is_err());
}

struct ContinuingBackend {
    steps: Arc<Mutex<Vec<fn64_render::RenderTaskStep>>>,
    dropped: Arc<AtomicBool>,
}

impl Drop for ContinuingBackend {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl fn64_render::RenderBackend for ContinuingBackend {
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
        panic!("continuing backend must enter through process_task_chunk")
    }

    fn process_task_chunk(
        &mut self,
        _rdram: &mut [u8],
        _rsp_memory: &mut fn64_runtime::RspMemory,
        _task: &fn64_render::OsTask,
        _output_addr: u32,
        step: fn64_render::RenderTaskStep,
    ) -> Result<fn64_render::RenderTaskChunkStatus, fn64_render::RenderError> {
        self.steps.lock().unwrap().push(step);
        match step {
            fn64_render::RenderTaskStep::Start => Ok(fn64_render::RenderTaskChunkStatus::Continue(
                fn64_render::RenderTaskContinuation::new(1),
            )),
            fn64_render::RenderTaskStep::Resume(token) => {
                panic!("terminal seal unexpectedly resumed token {}", token.get())
            }
        }
    }

    fn task_chunking(&self) -> fn64_render::RenderTaskChunking {
        fn64_render::RenderTaskChunking::Resumable
    }

    fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
        fn64_render::DpFullSyncStatus::NotReached
    }

    fn present(
        &mut self,
        _request: fn64_render::PresentRequest<'_>,
    ) -> Result<(), fn64_render::RenderError> {
        Ok(())
    }

    fn resize(&mut self, _width: u32, _height: u32) {}

    fn supported_ucodes(&self) -> &[fn64_render::UcodeId] {
        &[]
    }
}

fn run_render_continuation_child() {
    const HEADER_OFF: usize = 0x40;
    const BOOT_OFF: usize = 0x400;
    const UCODE_OFF: usize = BOOT_OFF + 32;

    fn64_abi::load_rom(Vec::new());
    let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
    rdram[HEADER_OFF..HEADER_OFF + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
    let mtc0 = |rt: u32, rd: u32| (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11);
    let boot = [
        0x2402_0000 | UCODE_OFF as u32,
        mtc0(2, 1),
        0x2403_1080,
        mtc0(3, 0),
        0x2404_0007,
        mtc0(4, 2),
        0x0800_0020,
        0x2407_7777,
    ];
    for (index, word) in boot.into_iter().enumerate() {
        let offset = BOOT_OFF + index * 4;
        rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
    }
    for (field, value) in [
        (0x08, BOOT_OFF as u32),
        (0x0c, 32),
        (0x10, UCODE_OFF as u32),
        (0x14, 8),
    ] {
        rdram[HEADER_OFF + field..HEADER_OFF + field + 4].copy_from_slice(&value.to_ne_bytes());
    }

    unsafe { fn64_abi::register_process_rdram(rdram.as_mut_ptr(), rdram.len()) };
    let steps = Arc::new(Mutex::new(Vec::new()));
    let dropped = Arc::new(AtomicBool::new(false));
    fn64_abi::set_render_backend(
        Box::new(ContinuingBackend {
            steps: Arc::clone(&steps),
            dropped: Arc::clone(&dropped),
        }),
        rdram.len(),
    );

    let mut ctx = fn64_abi::RecompContext::zeroed();
    ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;
    unsafe { fn64_abi::osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };
    unsafe { fn64_abi::osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };
    assert_eq!(
        steps.lock().unwrap().as_slice(),
        [fn64_render::RenderTaskStep::Start]
    );
    assert_eq!(fn64_abi::next_device_deadline(), Some(fn64_abi::sim_time()));

    let summary = fn64_abi::prepare_process_exit();
    assert_eq!(summary.threads, 0);
    assert_eq!(summary.detached_coroutines, 0);
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(
        steps.lock().unwrap().as_slice(),
        [fn64_render::RenderTaskStep::Start],
        "terminal seal must discard, not resume, retained renderer work"
    );
}

fn run_child(test_name: &str, child_mode: &str) {
    let output = Command::new(std::env::current_exe().expect("locate process-exit test binary"))
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(CHILD_ENV, child_mode)
        .output()
        .expect("launch process-exit child");
    assert!(
        output.status.success(),
        "sealed child did not exit normally: status={}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn suspended_extern_c_coroutine_allows_normal_process_exit_after_terminal_seal() {
    if std::env::var_os(CHILD_ENV).as_deref() == Some(BLOCKED_CHILD.as_ref()) {
        run_blocked_coroutine_child();
        return;
    }

    run_child(
        "suspended_extern_c_coroutine_allows_normal_process_exit_after_terminal_seal",
        BLOCKED_CHILD,
    );
}

#[test]
fn retained_render_continuation_allows_normal_process_exit_without_resume() {
    if std::env::var_os(CHILD_ENV).as_deref() == Some(RENDER_CHILD.as_ref()) {
        run_render_continuation_child();
        return;
    }

    run_child(
        "retained_render_continuation_allows_normal_process_exit_without_resume",
        RENDER_CHILD,
    );
}
