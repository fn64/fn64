//! Non-game fixed-cycle exercise of the live runtime/device/render release gate.

#[cfg(feature = "synthetic-native-archive-evidence")]
extern crate fn64_render_rt64 as _fn64_render_rt64;

#[cfg(feature = "synthetic-native-archive-evidence")]
use fn64_boot_harness::NativeProgramArtifactIdentity;
use fn64_boot_harness::{
    commit_scheduled_vi_boundary_with_program, parse_unsupported_journal,
    verify_release_report_journal, LiveReferenceFramebufferEvidence, LiveReleaseGate,
    LiveReleaseGateObservationExt as _, ReleaseProgramDescriptor,
    REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES,
};
use fn64_render::{
    FrameStatus, NonRdpWrite16, NonRdpWrite16Disposition, OsTask, RenderBackend, RenderConfig,
    RenderError, UcodeId,
};
use fn64_render_reference::ReferenceBackend;
use fn64_runtime::{M_AUDTASK, M_GFXTASK};
use std::{
    env,
    error::Error,
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const QUEUE: usize = 0x100;
const MESSAGE_STORAGE: usize = 0x140;
const IO_MESSAGE: usize = 0x180;
const CONTROLLER_QUEUE: usize = 0x1c0;
const CONTROLLER_MESSAGE_STORAGE: usize = 0x1e0;
const STACK: usize = 0x200;
const PI_TARGET: usize = 0x300;
const SI_BUFFER: usize = 0x400;
const AI_BUFFER: usize = 0x500;
const CONTROLLER_PATTERN: usize = 0x580;
const CONTROLLER_STATUS: usize = 0x590;
const CONTROLLER_PAD: usize = 0x5c0;
const GFX_TASK: usize = 0x600;
const AUDIO_TASK: usize = 0x680;
const GFX_RSP_UCODE: usize = 0x700;
const AUDIO_RSP_BOOT: usize = 0x740;
const RSP_UCODE_DATA: usize = 0x780;
const DRAM_RDP_COMMANDS: usize = 0x800;
const XBUS_RDP_COMMANDS: usize = 0x840;
const FRAMEBUFFER: usize = 0x900;
const RSP_COMPLETION_QUEUE: usize = 0x1000;
const RSP_COMPLETION_STORAGE: usize = 0x1020;
const AUDIO_EXECUTION_MARKER: usize = 0x1040;
const AUDIO_EXECUTION_MAGIC: u32 = 0x4155_4449;
const OS_EVENT_SP: u64 = 4;
const OS_EVENT_SI: u64 = 5;
const RSP_DONE_MESSAGE: u64 = 0x5253_5044;
const CONTROLLER_DONE_MESSAGE: u64 = 0x5349_444f;
const WIDTH: usize = 4;
const HEIGHT: usize = 2;
#[cfg(feature = "synthetic-native-archive-evidence")]
const SYNTHETIC_GENERATED_ARCHIVE: &[u8] = include_bytes!(env!("FN64_SYNTHETIC_GENERATED_ARCHIVE"));
#[cfg(feature = "synthetic-native-archive-evidence")]
const SYNTHETIC_BRIDGE_ARCHIVE: &[u8] = include_bytes!(env!("FN64_SYNTHETIC_BRIDGE_ARCHIVE"));

#[cfg(feature = "synthetic-native-archive-evidence")]
#[allow(improper_ctypes)]
unsafe extern "C" {
    fn fn64_synthetic_recompiled_entry(rdram: *mut u8, ctx: *mut fn64_abi::RecompContext);
    fn fn64_synthetic_recompiled_step(value: u32) -> u32;
    fn fn64_synthetic_section_bridge(value: u32) -> u32;
}

fn main() {
    if let Err(error) = run() {
        eprintln!("synthetic-fixed-cycle-release: {error}");
        std::process::exit(1);
    }
}

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    const USAGE: &str = "usage: set FN64_RELEASE_GATE_CYCLE, FN64_RELEASE_REPORT, and FN64_RELEASE_RUN_EVENT_SHA256, or pass REPORT.json JOURNAL.jsonl RUN_EVENT_SHA256";
    let release_environment = fn64_boot_harness::release_run_environment_from_process()?;
    let mut arguments = env::args_os().skip(1);
    let invocation = if release_environment.is_some() {
        if arguments.next().is_some() {
            return Err(io::Error::other(format!(
                "{USAGE}; arguments cannot accompany FN64_RELEASE_*"
            ))
            .into());
        }
        return run_from_release_environment();
    } else {
        let report_path = arguments
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other(USAGE))?;
        let journal_path = arguments
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other(USAGE))?;
        let run_event_sha256 = arguments
            .next()
            .ok_or_else(|| io::Error::other(USAGE))?
            .into_string()
            .map_err(|_| io::Error::other("RUN_EVENT_SHA256 must be UTF-8"))?;
        if arguments.next().is_some() {
            return Err(io::Error::other(USAGE).into());
        }
        ReleaseInvocation {
            report_path,
            journal_path,
            run_event_sha256,
            expected_cycle: None,
        }
    };

    run_invocation(invocation)
}

/// Fresh-process entry point for a Rust test harness launched by the trusted
/// private-series runner. The normal example path rejects all process
/// arguments in runner mode, while the test harness necessarily has its own.
pub(crate) fn run_from_release_environment() -> Result<(), Box<dyn Error>> {
    let environment = fn64_boot_harness::release_run_environment_from_process()?
        .ok_or_else(|| io::Error::other("FN64_RELEASE_* runner environment is absent"))?;
    run_invocation(ReleaseInvocation::from_environment(environment))
}

struct ReleaseInvocation {
    report_path: PathBuf,
    journal_path: PathBuf,
    run_event_sha256: String,
    expected_cycle: Option<u64>,
}

impl ReleaseInvocation {
    fn from_environment(environment: fn64_boot_harness::ReleaseRunEnvironment) -> Self {
        Self {
            report_path: environment.report_path.clone(),
            journal_path: environment.journal_path(),
            run_event_sha256: environment.run_event_sha256,
            expected_cycle: Some(environment.guest_cycle),
        }
    }
}

fn run_invocation(invocation: ReleaseInvocation) -> Result<(), Box<dyn Error>> {
    let ReleaseInvocation {
        report_path,
        journal_path,
        run_event_sha256,
        expected_cycle,
    } = invocation;

    let mut rdram = vec![0u8; RDRAM_LEN];
    prepare_synthetic_memory(&mut rdram);
    fn64_abi::load_rom(REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES.to_vec());
    let (program, scenario) = release_program()?;
    fn64_abi::configure_no_cartridge_save();
    fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
    fn64_abi::set_audio_rdram_len(rdram.len());
    unsafe { fn64_abi::set_audio_ucode_fn(synthetic_audio_ucode) };
    let fixed_cycle = fn64_abi::next_vi_deadline()
        .ok_or_else(|| io::Error::other("NTSC configuration did not schedule a VI edge"))?;
    if let Some(expected_cycle) = expected_cycle {
        if fixed_cycle != expected_cycle {
            return Err(io::Error::other(format!(
                "FN64_RELEASE_GATE_CYCLE selected {expected_cycle}, synthetic scheduled VI edge is {fixed_cycle}"
            ))
            .into());
        }
    }

    let mut gate = LiveReleaseGate::new(fixed_cycle);
    gate.arm_with_unsupported_journal(&journal_path, &run_event_sha256)?;
    let presents = Arc::new(AtomicU64::new(0));
    let mut reference = ReferenceBackend::new().with_f3dex2();
    reference.create(&RenderConfig::ntsc(WIDTH as u32, HEIGHT as u32))?;
    fn64_abi::set_render_backend_with_policy(
        Box::new(ObservedReferenceBackend {
            inner: reference,
            presents: Arc::clone(&presents),
        }),
        rdram.len(),
        fn64_abi::GraphicsTaskExecutionPolicy::LleAccuracy,
    );

    unsafe {
        fn64_abi::boot_thread0(rdram.as_mut_ptr(), rdram.len(), synthetic_entry, 1, 10);
    }
    fn64_abi::run_to_idle();
    let audio_execution = u32::from_ne_bytes(
        rdram[AUDIO_EXECUTION_MARKER..AUDIO_EXECUTION_MARKER + 4]
            .try_into()
            .expect("synthetic audio execution marker is one word"),
    );
    if audio_execution != AUDIO_EXECUTION_MAGIC ^ AUDIO_TASK as u32 {
        return Err(io::Error::other(format!(
            "synthetic audio task did not execute its registered ucode: marker={audio_execution:#010x}"
        ))
        .into());
    }
    let boundary = commit_scheduled_vi_boundary_with_program(fixed_cycle, program)?;
    let present_count = presents.load(Ordering::SeqCst);
    if present_count != 1 {
        return Err(io::Error::other(format!(
            "scheduled VI edge produced {present_count} registered-backend presents instead of one"
        ))
        .into());
    }
    if fn64_abi::current_vi_framebuffer() != Some(FRAMEBUFFER as u32) {
        return Err(
            io::Error::other("scheduled VI edge did not latch the synthetic framebuffer").into(),
        );
    }
    if let Some(error) = fn64_abi::last_render_error() {
        return Err(io::Error::other(format!("registered render path failed: {error}")).into());
    }
    let mut framebuffer_bytes = vec![0; WIDTH * HEIGHT * 2];
    fn64_runtime::RdramView::from_storage(&rdram).copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(FRAMEBUFFER as u32),
        &mut framebuffer_bytes,
    );
    let framebuffer = LiveReferenceFramebufferEvidence::rgba16(
        FRAMEBUFFER as u32,
        WIDTH as u32,
        HEIGHT as u32,
        framebuffer_bytes,
    )?;
    let report = gate.capture_and_write_reference_evidence(
        boundary,
        scenario,
        REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES,
        &framebuffer,
        &report_path,
    )?;

    let journal = parse_unsupported_journal(&std::fs::read(&journal_path)?)?;
    verify_release_report_journal(&report, &journal)?;
    println!(
        "cycle={} report_sha256={} artifact_root_sha256={}",
        fixed_cycle, report.report_sha256, report.digest.root_sha256
    );
    Ok(())
}

#[cfg(not(feature = "synthetic-native-archive-evidence"))]
fn release_program() -> Result<(ReleaseProgramDescriptor, &'static str), Box<dyn Error>> {
    Ok((
        ReleaseProgramDescriptor::NoProgram,
        "synthetic-runtime-device-render-fixed-cycle-v1",
    ))
}

#[cfg(feature = "synthetic-native-archive-evidence")]
fn release_program() -> Result<(ReleaseProgramDescriptor, &'static str), Box<dyn Error>> {
    let declared =
        NativeProgramArtifactIdentity::from_hex(env!("FN64_SYNTHETIC_NATIVE_PROGRAM_SHA256"))?;
    let entries = || {
        [
            (
                "synthetic-generated-code".to_owned(),
                SYNTHETIC_GENERATED_ARCHIVE.to_vec(),
            ),
            (
                "synthetic-section-bridge".to_owned(),
                SYNTHETIC_BRIDGE_ARCHIVE.to_vec(),
            ),
        ]
    };
    let observed = NativeProgramArtifactIdentity::new(
        fn64_boot_harness::native_program_archives_sha256(entries()),
    );
    if observed != declared {
        return Err(io::Error::other(
            "embedded synthetic native archives do not match their build-time identity",
        )
        .into());
    }
    for archive_index in 0..2 {
        let mut mutated = entries();
        let bytes = &mut mutated[archive_index].1;
        if bytes.is_empty() {
            return Err(io::Error::other("synthetic native archive is empty").into());
        }
        let mutation_index = bytes.len() / 2;
        bytes[mutation_index] ^= 1;
        let mutated_identity = NativeProgramArtifactIdentity::new(
            fn64_boot_harness::native_program_archives_sha256(mutated),
        );
        if mutated_identity == declared {
            return Err(io::Error::other(format!(
                "synthetic native archive byte mutation {archive_index} retained its identity"
            ))
            .into());
        }
    }

    // SAFETY: both symbols are compiled by this package's opt-in build fixture
    // with the exact fixed-width signatures declared above.
    let (generated, bridge) = unsafe {
        (
            fn64_synthetic_recompiled_step(0x1234_5678),
            fn64_synthetic_section_bridge(0x89ab_cdef),
        )
    };
    if generated != 0x5d04_652e || bridge != 0xd5e6_f7c4 {
        return Err(io::Error::other(format!(
            "linked synthetic native archives returned {generated:#010x}, {bridge:#010x}"
        ))
        .into());
    }
    unsafe {
        fn64_abi::register_section(
            0,
            0x8000_1000,
            4,
            &[(0, 4, fn64_synthetic_recompiled_entry)],
        );
    }
    Ok((
        ReleaseProgramDescriptor::NativeArchive(declared),
        "synthetic-native-archive-runtime-device-render-fixed-cycle-v1",
    ))
}

fn prepare_synthetic_memory(rdram: &mut [u8]) {
    for (index, byte) in rdram[AI_BUFFER..AI_BUFFER + 32].iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(13).wrapping_add(7);
    }
    let mtc0 = |rt: u32, rd: u32| (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11);
    let mfc0 = |rt: u32, rd: u32| (0x10 << 26) | (rt << 16) | (rd << 11);
    let bne = |rs: u32, rt: u32, offset: i16| {
        (0x05 << 26) | (rs << 21) | (rt << 16) | u32::from(offset as u16)
    };
    let gfx_ucode = [
        0x2408_0000,
        mtc0(8, 0),
        0x2408_0000 | XBUS_RDP_COMMANDS as u32,
        mtc0(8, 1),
        0x2408_0027,
        mtc0(8, 2),
        mfc0(8, 6),
        bne(8, 0, -2),
        0x0000_0000,
        0x2408_0002,
        mtc0(8, 11),
        0x2408_0000,
        mtc0(8, 8),
        0x2408_0028,
        mtc0(8, 9),
        0x0000_000d,
    ];
    write_task(
        rdram,
        GFX_TASK,
        M_GFXTASK,
        GFX_RSP_UCODE,
        gfx_ucode.len() * 4,
    );
    write_task(rdram, AUDIO_TASK, M_AUDTASK, AUDIO_RSP_BOOT, 8);
    for (index, word) in gfx_ucode.into_iter().enumerate() {
        write_word(rdram, GFX_RSP_UCODE + index * 4, word);
    }
    write_word(rdram, AUDIO_RSP_BOOT, 0x0000_000d);
    write_word(rdram, AUDIO_RSP_BOOT + 4, 0);
    write_word(rdram, RSP_UCODE_DATA, 0x666e_3634);
    write_word(rdram, RSP_UCODE_DATA + 4, 0x6461_7461);

    let commands: [(u32, u32); 5] = [
        (0xef00_0000 | (3 << 20), 0),
        (0xff10_0003, FRAMEBUFFER as u32),
        (0xf700_0000, 0x39cf_39cf),
        (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
        (0xe900_0000, 0),
    ];
    for base in [DRAM_RDP_COMMANDS, XBUS_RDP_COMMANDS] {
        for (index, (word0, word1)) in commands.into_iter().enumerate() {
            write_word(rdram, base + index * 8, word0);
            write_word(rdram, base + index * 8 + 4, word1);
        }
    }
}

fn write_task(rdram: &mut [u8], base: usize, task_type: u32, ucode: usize, ucode_bytes: usize) {
    write_word(rdram, base, task_type);
    write_word(rdram, base + 0x08, kseg(ucode));
    write_word(rdram, base + 0x0c, ucode_bytes as u32);
    write_word(rdram, base + 0x10, kseg(ucode));
    write_word(rdram, base + 0x14, ucode_bytes as u32);
    write_word(rdram, base + 0x18, kseg(RSP_UCODE_DATA));
    write_word(rdram, base + 0x1c, 8);
}

unsafe extern "C" fn synthetic_entry(rdram: *mut u8, ctx: *mut fn64_abi::RecompContext) {
    let ctx = unsafe { &mut *ctx };

    #[cfg(feature = "synthetic-native-archive-evidence")]
    unsafe {
        fn64_synthetic_recompiled_entry(rdram, ctx);
    }

    ctx.r4 = kseg(QUEUE) as u64;
    ctx.r5 = kseg(MESSAGE_STORAGE) as u64;
    ctx.r6 = 4;
    unsafe { fn64_abi::osCreateMesgQueue_recomp(rdram, ctx) };
    ctx.r4 = kseg(QUEUE) as u64;
    ctx.r5 = 0x5359_4e54;
    ctx.r6 = 0;
    unsafe { fn64_abi::osSendMesg_recomp(rdram, ctx) };

    ctx.r4 = kseg(RSP_COMPLETION_QUEUE) as u64;
    ctx.r5 = kseg(RSP_COMPLETION_STORAGE) as u64;
    ctx.r6 = 1;
    unsafe { fn64_abi::osCreateMesgQueue_recomp(rdram, ctx) };
    ctx.r4 = OS_EVENT_SP;
    ctx.r5 = kseg(RSP_COMPLETION_QUEUE) as u64;
    ctx.r6 = RSP_DONE_MESSAGE;
    unsafe { fn64_abi::osSetEventMesg_recomp(rdram, ctx) };

    ctx.r4 = kseg(CONTROLLER_QUEUE) as u64;
    ctx.r5 = kseg(CONTROLLER_MESSAGE_STORAGE) as u64;
    ctx.r6 = 1;
    unsafe { fn64_abi::osCreateMesgQueue_recomp(rdram, ctx) };
    ctx.r4 = OS_EVENT_SI;
    ctx.r5 = kseg(CONTROLLER_QUEUE) as u64;
    ctx.r6 = CONTROLLER_DONE_MESSAGE;
    unsafe { fn64_abi::osSetEventMesg_recomp(rdram, ctx) };
    ctx.r4 = kseg(CONTROLLER_QUEUE) as u64;
    ctx.r5 = kseg(CONTROLLER_PATTERN) as u64;
    ctx.r6 = kseg(CONTROLLER_STATUS) as u64;
    unsafe { fn64_abi::osContInit_recomp(rdram, ctx) };
    assert_eq!(ctx.r2, 0, "synthetic controller initialization failed");
    ctx.r4 = kseg(CONTROLLER_QUEUE) as u64;
    unsafe { fn64_abi::osContStartReadData_recomp(rdram, ctx) };
    assert_eq!(ctx.r2, 0, "synthetic controller read start failed");
    ctx.r4 = kseg(CONTROLLER_QUEUE) as u64;
    ctx.r5 = 0;
    ctx.r6 = 1;
    unsafe { fn64_abi::osRecvMesg_recomp(rdram, ctx) };
    assert_eq!(ctx.r2, 0, "synthetic controller completion wait failed");
    ctx.r4 = kseg(CONTROLLER_PAD) as u64;
    unsafe { fn64_abi::osContGetReadData_recomp(rdram, ctx) };

    ctx.r4 = 10;
    unsafe { fn64_abi::osCreateViManager_recomp(rdram, ctx) };
    ctx.r4 = kseg(FRAMEBUFFER) as u64;
    unsafe { fn64_abi::osViSwapBuffer_recomp(rdram, ctx) };
    ctx.r4 = kseg(DRAM_RDP_COMMANDS) as u64;
    ctx.r6 = 0;
    ctx.r7 = (4 * 8) as u64;
    unsafe { fn64_abi::osDpSetNextBuffer_recomp(rdram, ctx) };
    assert_eq!(ctx.r2, 0, "synthetic raw RDP submission was rejected");

    write_raw_word(rdram, STACK + 0x10, kseg(PI_TARGET));
    write_raw_word(rdram, STACK + 0x14, 16);
    write_raw_word(rdram, STACK + 0x18, kseg(QUEUE));
    ctx.r4 = kseg(IO_MESSAGE) as u64;
    ctx.r5 = 0;
    ctx.r6 = 0;
    ctx.r7 = 0;
    ctx.r29 = kseg(STACK) as u64;
    unsafe { fn64_abi::osPiStartDma_recomp(rdram, ctx) };

    ctx.r4 = 1;
    ctx.r5 = kseg(SI_BUFFER) as u64;
    unsafe { fn64_abi::__osSiRawStartDma_recomp(rdram, ctx) };

    ctx.r4 = kseg(AI_BUFFER) as u64;
    ctx.r5 = 32;
    unsafe { fn64_abi::osAiSetNextBuffer_recomp(rdram, ctx) };

    ctx.r4 = kseg(AUDIO_TASK) as u64;
    unsafe { fn64_abi::osSpTaskLoad_recomp(rdram, ctx) };
    unsafe { fn64_abi::osSpTaskStartGo_recomp(rdram, ctx) };
    ctx.r4 = kseg(RSP_COMPLETION_QUEUE) as u64;
    ctx.r5 = 0;
    ctx.r6 = 1;
    unsafe { fn64_abi::osRecvMesg_recomp(rdram, ctx) };
    assert_eq!(ctx.r2, 0, "synthetic audio task completion wait failed");

    ctx.r4 = kseg(GFX_TASK) as u64;
    unsafe { fn64_abi::osSpTaskLoad_recomp(rdram, ctx) };
    unsafe { fn64_abi::osSpTaskStartGo_recomp(rdram, ctx) };
    ctx.r4 = kseg(RSP_COMPLETION_QUEUE) as u64;
    ctx.r5 = 0;
    ctx.r6 = 1;
    unsafe { fn64_abi::osRecvMesg_recomp(rdram, ctx) };
    assert_eq!(ctx.r2, 0, "synthetic graphics task completion wait failed");
}

unsafe extern "C" fn synthetic_audio_ucode(rdram: *mut u8, task_offset: u32) -> u32 {
    let marker = AUDIO_EXECUTION_MAGIC ^ task_offset;
    unsafe {
        std::ptr::copy_nonoverlapping(
            marker.to_ne_bytes().as_ptr(),
            rdram.add(AUDIO_EXECUTION_MARKER),
            4,
        );
    }
    0
}

fn write_word(rdram: &mut [u8], offset: usize, value: u32) {
    rdram[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn write_raw_word(rdram: *mut u8, offset: usize, value: u32) {
    unsafe {
        std::ptr::copy_nonoverlapping(value.to_ne_bytes().as_ptr(), rdram.add(offset), 4);
    }
}

const fn kseg(offset: usize) -> u32 {
    0x8000_0000 | offset as u32
}

struct ObservedReferenceBackend {
    inner: ReferenceBackend,
    presents: Arc<AtomicU64>,
}

impl RenderBackend for ObservedReferenceBackend {
    fn release_environment(&self) -> fn64_render::RenderBackendEvidence {
        self.inner.release_environment()
    }

    fn create(&mut self, config: &RenderConfig) -> Result<(), RenderError> {
        self.inner.create(config)
    }

    fn observe_non_rdp_write16(&mut self, write: NonRdpWrite16) -> NonRdpWrite16Disposition {
        self.inner.observe_non_rdp_write16(write)
    }

    fn process_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &OsTask,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        self.inner
            .process_task(rdram, rsp_memory, task, output_addr)
    }

    fn process_rdp_commands(
        &mut self,
        rdram: &mut [u8],
        start: u32,
        end: u32,
        output_addr: u32,
    ) -> Result<FrameStatus, RenderError> {
        self.inner
            .process_rdp_commands(rdram, start, end, output_addr)
    }

    fn present(&mut self, request: fn64_render::PresentRequest<'_>) -> Result<(), RenderError> {
        self.inner.present(request)?;
        self.presents.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.inner.resize(width, height);
    }

    fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
        self.inner.last_dp_full_sync()
    }

    fn supported_ucodes(&self) -> &[UcodeId] {
        self.inner.supported_ucodes()
    }
}
