//! Non-game fixed-cycle exercise of the live runtime/device/render release gate.

#[cfg(feature = "synthetic-native-archive-evidence")]
use fn64_boot_harness::NativeProgramArtifactIdentity;
use fn64_boot_harness::{
    commit_scheduled_vi_boundary_with_program, parse_unsupported_journal,
    verify_release_report_journal, LiveMemoryEvidence, LiveReferenceFramebufferEvidence,
    LiveReleaseGate, LiveReleaseGateObservationExt as _, ReleaseProgramDescriptor,
};
use fn64_render::{
    FrameStatus, NonRdpWrite16, NonRdpWrite16Disposition, OsTask, RenderBackend, RenderConfig,
    RenderError, UcodeId,
};
use fn64_render_rt64::ReferenceBackend;
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
const STACK: usize = 0x200;
const PI_TARGET: usize = 0x300;
const SI_BUFFER: usize = 0x400;
const AI_BUFFER: usize = 0x500;
const GFX_TASK: usize = 0x600;
const AUDIO_TASK: usize = 0x680;
const RSP_BOOT: usize = 0x700;
const RDP_COMMANDS: usize = 0x800;
const FRAMEBUFFER: usize = 0x900;
const WIDTH: usize = 4;
const HEIGHT: usize = 2;
const SYNTHETIC_ROM: &[u8] = b"fn64 synthetic PI payload; not game content";

#[cfg(feature = "synthetic-native-archive-evidence")]
const SYNTHETIC_GENERATED_ARCHIVE: &[u8] = include_bytes!(env!("FN64_SYNTHETIC_GENERATED_ARCHIVE"));
#[cfg(feature = "synthetic-native-archive-evidence")]
const SYNTHETIC_BRIDGE_ARCHIVE: &[u8] = include_bytes!(env!("FN64_SYNTHETIC_BRIDGE_ARCHIVE"));

#[cfg(feature = "synthetic-native-archive-evidence")]
unsafe extern "C" {
    fn fn64_synthetic_recompiled_step(value: u32) -> u32;
    fn fn64_synthetic_section_bridge(value: u32) -> u32;
}

fn main() {
    if let Err(error) = run() {
        eprintln!("synthetic-fixed-cycle-release: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let report_path = arguments.next().map(PathBuf::from).ok_or_else(|| {
        io::Error::other(
            "usage: synthetic_fixed_cycle_release REPORT.json JOURNAL.jsonl RUN_EVENT_SHA256",
        )
    })?;
    let journal_path = arguments.next().map(PathBuf::from).ok_or_else(|| {
        io::Error::other(
            "usage: synthetic_fixed_cycle_release REPORT.json JOURNAL.jsonl RUN_EVENT_SHA256",
        )
    })?;
    let run_event_sha256 = arguments.next().ok_or_else(|| {
        io::Error::other(
            "usage: synthetic_fixed_cycle_release REPORT.json JOURNAL.jsonl RUN_EVENT_SHA256",
        )
    })?;
    let run_event_sha256 = run_event_sha256
        .into_string()
        .map_err(|_| io::Error::other("RUN_EVENT_SHA256 must be UTF-8"))?;
    if arguments.next().is_some() {
        return Err(io::Error::other(
            "usage: synthetic_fixed_cycle_release REPORT.json JOURNAL.jsonl RUN_EVENT_SHA256",
        )
        .into());
    }

    let (program, scenario) = release_program()?;
    let mut rdram = vec![0u8; RDRAM_LEN];
    prepare_synthetic_memory(&mut rdram);
    fn64_abi::load_rom(SYNTHETIC_ROM.to_vec());
    fn64_abi::configure_no_cartridge_save();
    fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
    fn64_abi::set_audio_rdram_len(rdram.len());
    let fixed_cycle = fn64_abi::next_vi_deadline()
        .ok_or_else(|| io::Error::other("NTSC configuration did not schedule a VI edge"))?;

    let mut gate = LiveReleaseGate::new(fixed_cycle);
    gate.arm_with_unsupported_journal(&journal_path, &run_event_sha256)?;
    let presents = Arc::new(AtomicU64::new(0));
    let mut reference = ReferenceBackend::new().with_f3dex2();
    reference.create(&RenderConfig::new(WIDTH as u32, HEIGHT as u32))?;
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
    let framebuffer = LiveReferenceFramebufferEvidence::rgba16(
        FRAMEBUFFER as u32,
        WIDTH as u32,
        HEIGHT as u32,
        rdram[FRAMEBUFFER..FRAMEBUFFER + WIDTH * HEIGHT * 2].to_vec(),
    )?;
    let memory = LiveMemoryEvidence::full_physical_rdram(rdram[..RDRAM_LEN].to_vec())?;
    let report = gate.capture_and_write_reference_evidence(
        boundary,
        scenario,
        b"fn64 synthetic non-game release input v1",
        &framebuffer,
        &memory,
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
    Ok((
        ReleaseProgramDescriptor::NativeArchive(declared),
        "synthetic-native-archive-runtime-device-render-fixed-cycle-v1",
    ))
}

fn prepare_synthetic_memory(rdram: &mut [u8]) {
    for (index, byte) in rdram[AI_BUFFER..AI_BUFFER + 32].iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(13).wrapping_add(7);
    }
    for task in [(GFX_TASK, M_GFXTASK), (AUDIO_TASK, M_AUDTASK)] {
        write_task(rdram, task.0, task.1);
    }
    write_word(rdram, RSP_BOOT, 0x0000_000d);
    write_word(rdram, RSP_BOOT + 4, 0);

    let commands: [(u32, u32); 5] = [
        (0xef00_0000 | (3 << 20), 0),
        (0xff10_0003, FRAMEBUFFER as u32),
        (0xf700_0000, 0x39cf_39cf),
        (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
        (0xe900_0000, 0),
    ];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        write_word(rdram, RDP_COMMANDS + index * 8, word0);
        write_word(rdram, RDP_COMMANDS + index * 8 + 4, word1);
    }
}

fn write_task(rdram: &mut [u8], base: usize, task_type: u32) {
    write_word(rdram, base, task_type);
    write_word(rdram, base + 0x08, kseg(RSP_BOOT));
    write_word(rdram, base + 0x0c, 8);
    write_word(rdram, base + 0x10, kseg(RSP_BOOT));
    write_word(rdram, base + 0x14, 8);
}

unsafe extern "C" fn synthetic_entry(rdram: *mut u8, ctx: *mut fn64_abi::RecompContext) {
    let ctx = unsafe { &mut *ctx };

    ctx.r4 = kseg(QUEUE) as u64;
    ctx.r5 = kseg(MESSAGE_STORAGE) as u64;
    ctx.r6 = 4;
    unsafe { fn64_abi::osCreateMesgQueue_recomp(rdram, ctx) };
    ctx.r4 = kseg(QUEUE) as u64;
    ctx.r5 = 0x5359_4e54;
    ctx.r6 = 0;
    unsafe { fn64_abi::osSendMesg_recomp(rdram, ctx) };

    ctx.r4 = 10;
    unsafe { fn64_abi::osCreateViManager_recomp(rdram, ctx) };
    ctx.r4 = kseg(FRAMEBUFFER) as u64;
    unsafe { fn64_abi::osViSwapBuffer_recomp(rdram, ctx) };
    ctx.r4 = kseg(RDP_COMMANDS) as u64;
    ctx.r6 = 0;
    ctx.r7 = (5 * 8) as u64;
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

    for task in [GFX_TASK, AUDIO_TASK] {
        ctx.r4 = kseg(task) as u64;
        unsafe { fn64_abi::osSpTaskLoad_recomp(rdram, ctx) };
    }
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

    fn present(&mut self, vi: fn64_render::ViPresentation) -> Result<(), RenderError> {
        self.inner.present(vi)?;
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
