
use super::*;
use fn64_runtime::{
    AiDmaRequest, Cycles, DeviceEvidenceSnapshot, DeviceSnapshot, OsTaskHeader, PiDmaRequest,
    PiDomainTiming, RdramAddr, RspMemAddr, SaveOperationKind, SiDmaRequest, TvType,
    RSP_MEMORY_BANK_SIZE,
};

fn encode_device_snapshot(
    snapshot: DeviceEvidenceSnapshot,
    executor: fn64_runtime::ExecutorControlEvidenceSnapshot,
    host: fn64_abi::AbiHostEvidenceSnapshot,
    program: crate::ProgramEvidenceSnapshot,
) -> Vec<u8> {
    try_encode_device_snapshot(snapshot, executor, host, program)
        .expect("test device evidence must be canonical")
}

#[cfg(feature = "recomp-rs")]
fn publication_cpu_snapshot(seed: u64) -> fn64_cpu_runtime::RecompContextEvidenceSnapshotV1 {
    let mut gprs = [0_u64; 32];
    let mut physical_fgrs = [0_u64; 32];
    for (index, value) in gprs.iter_mut().enumerate() {
        *value = seed.wrapping_add(index as u64);
    }
    for (index, value) in physical_fgrs.iter_mut().enumerate() {
        *value = seed.wrapping_mul(3).wrapping_add(index as u64);
    }
    let mut tlb_entries = [fn64_cpu_runtime::TlbEntryRaw::default(); 32];
    tlb_entries[0] = fn64_cpu_runtime::TlbEntryRaw {
        page_mask: seed as u32 ^ 0x0000_6000,
        entry_hi: seed ^ 0x1234_5678_9abc_def0,
        entry_lo0: seed as u32 ^ 0x1357_2468,
        entry_lo1: seed as u32 ^ 0x2468_1357,
    };
    tlb_entries[31] = fn64_cpu_runtime::TlbEntryRaw {
        page_mask: 0x01ff_e000,
        entry_hi: seed.rotate_left(17),
        entry_lo0: 0x1234,
        entry_lo1: 0x5678,
    };
    fn64_cpu_runtime::RecompContextEvidenceSnapshotV1 {
        gprs,
        hi: seed ^ 0x1111,
        lo: seed ^ 0x2222,
        physical_fgrs,
        fpu_cond: seed & 1 != 0,
        fcsr: seed as u32 ^ 0x0102_0304,
        ll_reservation: Some((seed ^ 0x8000_0000, 8)),
        cop0_count: seed as u32 + 1,
        cop0_compare: seed as u32 + 2,
        cop0_count_write: Some(seed as u32 + 3),
        cop0_compare_write: None,
        cop0_cond: seed & 2 != 0,
        cop0_status: seed as u32 ^ 0x0405_0607,
        cop0_cause: seed as u32 ^ 0x0809_0a0b,
        cop0_epc: seed as u32 ^ 0x8000_0100,
        cop0_error_epc: seed as u32 ^ 0x8000_0200,
        cop0_badvaddr: seed ^ 0xffff_ffff_8000_0300,
        cop0_context: seed as u32 ^ 0x0c0d_0e0f,
        cop0_xcontext: seed ^ 0x1011_1213_1415_1617,
        cop0_index: seed as u32 & 31,
        tlb_entries,
        cop0_entry_lo0: seed as u32 ^ 0x1819_1a1b,
        cop0_entry_lo1: seed as u32 ^ 0x1c1d_1e1f,
        cop0_page_mask: seed as u32 ^ 0x0020_2000,
        cop0_wired: seed as u32 & 31,
        cop0_entry_hi: seed ^ 0x2021_2223_2425_2627,
        cop0_random_phase: seed as u32 & 31,
        cop0_watch_lo: seed as u32 ^ 0x2829_2a2b,
        cop0_watch_hi: seed as u32 ^ 0x2c2d_2e2f,
        os_interrupt_mask: seed as u32 ^ 0x3031_3233,
        thread_return_pc: Some(seed as u32 ^ 0xffff_fffc),
    }
}

#[cfg(feature = "recomp-rs")]
fn publication_cpu_snapshot_without_pending_timing(
    seed: u64,
) -> fn64_cpu_runtime::RecompContextEvidenceSnapshotV1 {
    let mut snapshot = publication_cpu_snapshot(seed);
    snapshot.cop0_count_write = None;
    snapshot.cop0_compare_write = None;
    snapshot
}

#[cfg(feature = "recomp-rs")]
fn publication_key(bank: u64, pc: u32) -> fn64_cpu_runtime::ExecutionKey {
    fn64_cpu_runtime::ExecutionKey::new(
        fn64_cpu_runtime::BankId::new(bank),
        fn64_cpu_runtime::GuestPc::new(pc),
    )
}

#[cfg(feature = "recomp-rs")]
fn publication_digest_hex(digest: [u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn admission_generation(value: u64) -> fn64_abi::RspTaskAdmissionGeneration {
    fn64_abi::RspTaskAdmissionGeneration::new(
        std::num::NonZeroU64::new(value).expect("test admission generation must be nonzero"),
    )
}

fn observations() -> ReleaseObservationGeometry {
    ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap()
}

fn test_rom(destination_code: u8) -> Vec<u8> {
    let mut rom = vec![0; 0x1000];
    rom[..4].copy_from_slice(&MAGIC_Z64.to_be_bytes());
    rom[0x3b..0x3f].copy_from_slice(&[b'N', b'F', b'6', destination_code]);
    rom
}

fn n64_order(canonical: &[u8]) -> Vec<u8> {
    canonical
        .chunks_exact(4)
        .flat_map(|word| [word[3], word[2], word[1], word[0]])
        .collect()
}

fn v64_order(canonical: &[u8]) -> Vec<u8> {
    canonical
        .chunks_exact(2)
        .flat_map(|pair| [pair[1], pair[0]])
        .collect()
}

fn authoritative_rt64_identity_for(graphics_api: ReleaseGraphicsApi) -> String {
    let post_vi_api = match graphics_api {
        ReleaseGraphicsApi::D3d12 => "d3d12-bgra8-rgba8-unorm",
        ReleaseGraphicsApi::Vulkan => "vulkan-bgra8-rgba8-unorm",
        ReleaseGraphicsApi::Metal => "metal-bgra8-unorm",
    };
    format!(
        "adapter=fn64-render-rt64/rt64;adapter_sha256={};source=git:{};provenance=git-clean;overlay=test;post_vi_api={post_vi_api}",
        "a".repeat(64),
        "b".repeat(40),
    )
}

fn authoritative_rt64_identity() -> String {
    authoritative_rt64_identity_for(current_test_graphics_api())
}

fn current_test_graphics_api() -> ReleaseGraphicsApi {
    match crate::release_host_platform().unwrap() {
        ReleaseHostPlatform::MacosArm64 => ReleaseGraphicsApi::Metal,
        ReleaseHostPlatform::LinuxX86_64 => ReleaseGraphicsApi::Vulkan,
        ReleaseHostPlatform::WindowsX86_64 => ReleaseGraphicsApi::D3d12,
    }
}

fn snapshot(cycle: u64) -> DeviceEvidenceSnapshot {
    DeviceEvidenceSnapshot {
        guest: DeviceSnapshot {
            now: fn64_runtime::EmulatedInstant::new(cycle),
            pi_dram_addr: RdramAddr::from_offset(0x100),
            pi_cart_addr: 0x1000_1000,
            pi_status: 1,
            ai_status: 0,
            ai_length: 0x200,
            ai_dram_addr: RdramAddr::from_offset(0x400),
            ai_control: 1,
            ai_dacrate: 0x2ef,
            ai_bitrate: 0xf,
            si_dram_addr: RdramAddr::from_offset(0x200),
            si_status: 0,
            vi_current: 20,
            vi_intr: 2,
            vi_v_sync: 525,
            tv_type: Some(TvType::Ntsc),
            vi_field_interval: Some(Cycles::new(781_250)),
            sp_busy: false,
            sp_status: 1,
            sp_mem_addr: RspMemAddr::from_register(0x40),
            sp_dram_addr: RdramAddr::from_offset(0x300),
            sp_imem_generation: 2,
            dp_busy: false,
            dpc_start: 0x100,
            dpc_end: 0x180,
            dpc_current: 0x180,
            dpc_status: 0,
            dpc_clock: 0,
            dpc_busy: 0,
            dpc_pipe_busy: 0,
            dpc_tmem_busy: 0,
            pending_dpc: None,
            mi_pending: 8,
            mi_mask: 8,
            pi_domain1: PiDomainTiming::default(),
            pi_domain2: PiDomainTiming::default(),
        },
        pi_timing_policy: b"test-policy".to_vec(),
        pending_pi: None,
        current_ai: None,
        queued_ai: None,
        pending_dpc: None,
        pending_si: None,
        si_dma_error: false,
        si_latency: Cycles::new(1),
        pif_control_latency: Cycles::new(4_616),
        mi_interrupt_occurrences: [None; 6],
        pif_ram: [0; 64],
        rsp_dmem: [0; RSP_MEMORY_BANK_SIZE],
        rsp_imem: [0; RSP_MEMORY_BANK_SIZE],
        sp_rd_len: 0,
        sp_wr_len: 0,
        sp_pc: 0,
        sp_semaphore: false,
        active_sp_dma: None,
        queued_sp_dma: None,
        sp_dma_setup_cycles: Cycles::new(8),
        vi_registers: [0; 14],
        vi_epoch: fn64_runtime::EmulatedInstant::ZERO,
        pending_vi_token: None,
        pending_sp_token: None,
        pending_dp_token: None,
        scheduled_events: Vec::new(),
        next_event_sequence: 0,
        next_ai_dma_id: 1,
        save_bytes: None,
        pending_eeprom_write: None,
    }
}

fn peripherals_snapshot() -> fn64_abi::RuntimePeripheralEvidenceSnapshot {
    fn64_abi::RuntimePeripheralEvidenceSnapshot {
        peripherals: fn64_runtime::Peripherals::new().evidence_snapshot(),
        pending_pi_completions: Vec::new(),
        pending_si_completion: None,
        pending_host_interrupt_routes: Vec::new(),
        completed_pfs_is_plug: Vec::new(),
        vi: fn64_abi::AbiViEvidenceSnapshot {
            pending_mode: None,
            active_mode: None,
            pending_control: None,
            pending_x_scale_bits: None,
            pending_y_scale_bits: None,
            active_x_scale_bits: 1.0f32.to_bits(),
            active_y_scale_bits: 1.0f32.to_bits(),
        },
    }
}

fn executor_snapshot() -> fn64_runtime::ExecutorControlEvidenceSnapshot {
    fn64_runtime::Executor::new().control_evidence_snapshot()
}

fn host_snapshot() -> fn64_abi::AbiHostEvidenceSnapshot {
    let mut snapshot = fn64_abi::host_evidence_snapshot();
    snapshot.runtime_peripherals = peripherals_snapshot();
    snapshot
}

fn rsp_architectural_state(
    change: impl FnOnce(&mut fn64_audio::rsp::runtime::RspMachine<'_>),
) -> fn64_audio::rsp::runtime::RspArchitecturalState {
    let mut rdram = vec![0; 0x1000];
    let mut machine = fn64_audio::rsp::runtime::RspMachine::new(&mut rdram);
    change(&mut machine);
    machine.snapshot_architectural_state()
}

fn rsp_execution_state() -> fn64_runtime::RspExecutionState {
    fn64_runtime::RspExecutionState {
        pc: 0,
        sp_status: 0,
        sp_semaphore: false,
        sp_dma_mem_addr: RspMemAddr::from_register(0),
        sp_dma_dram_addr: RdramAddr::from_offset(0),
        sp_dma_read_length: 0,
        sp_dma_write_length: 0,
        dpc_start: 0,
        dpc_end: 0,
        dpc_current: 0,
        dpc_status: 0,
        dpc_clock: 0,
        dpc_busy: 0,
        dpc_pipe_busy: 0,
        dpc_tmem_busy: 0,
    }
}

fn encode_test_device(
    device: DeviceEvidenceSnapshot,
    peripherals: fn64_abi::RuntimePeripheralEvidenceSnapshot,
) -> Vec<u8> {
    let mut host = host_snapshot();
    host.runtime_peripherals = peripherals;
    encode_device_snapshot(
        device,
        executor_snapshot(),
        host,
        crate::ProgramEvidenceSnapshot::NoProgram,
    )
}

fn complete_digest() -> DeterministicDigest {
    let cycle = 42;
    let mut gate = FixedCycleDigestGate::new(cycle);
    gate.capture(cycle, ArtifactKind::Framebuffer, b"fb")
        .unwrap();
    gate.capture(cycle, ArtifactKind::Audio, b"audio").unwrap();
    gate.capture(
        cycle,
        ArtifactKind::Memory,
        &vec![0; crate::DEFAULT_RDRAM_SIZE],
    )
    .unwrap();
    gate.capture_device_snapshot(
        snapshot(cycle),
        executor_snapshot(),
        host_snapshot(),
        crate::ProgramEvidenceSnapshot::NoProgram,
    )
    .unwrap();
    gate.capture_timing_trace(cycle, &[]).unwrap();
    gate.finish().unwrap()
}

fn native_destination_event(
    cycle: u64,
    section_index: u32,
    function_offset: u32,
    link_vram: u32,
) -> fn64_abi::NativeExecutionDestinationEvent {
    fn64_abi::NativeExecutionDestinationEvent {
        at: Cycles::new(cycle),
        destination: fn64_abi::NativeExecutionDestination {
            section_index,
            function_offset,
            link_vram,
        },
    }
}

#[cfg(feature = "recomp-rs")]
fn typed_block_program() -> crate::ProgramEvidenceSnapshot {
    use fn64_abi::recompiled::RecompiledProgramEvidenceSnapshot;
    use fn64_cpu_runtime::{
        BankId, BlockProgramEvidenceSnapshot, CodeBankEvidenceSnapshot,
        CodeSpanEvidenceSnapshot, GuestPc, ProgramArtifactIdentity,
        ProgramIdentityEvidenceSnapshot, ProgramIdentitySource,
    };
    let identity = |byte| ProgramArtifactIdentity::new([byte; 32]);
    crate::ProgramEvidenceSnapshot::TypedRust(RecompiledProgramEvidenceSnapshot::Block {
        program: BlockProgramEvidenceSnapshot {
            identity: ProgramIdentityEvidenceSnapshot {
                identity: identity(0x31),
                source: ProgramIdentitySource::CanonicalBlockProgramSha256,
            },
            banks: vec![CodeBankEvidenceSnapshot {
                id: BankId::new(0x32),
                runner_artifact_identity: identity(0x33),
                spans: vec![CodeSpanEvidenceSnapshot {
                    vram_start: GuestPc::new(0x8000_1000),
                    words: vec![0],
                }],
            }],
            physical_banks: Vec::new(),
            mapped_aot: Vec::new(),
        },
        dispatch_artifact_identity: identity(0x34),
        instruction_budget: 100,
        executable_regions: Vec::new(),
        pending_executable_writes: Vec::new(),
    })
}

unsafe extern "C" fn late_native_destination(
    _rdram: *mut u8,
    _ctx: *mut fn64_abi::RecompContext,
) {
}

fn assert_noncanonical_dpc_counter_rejected(
    register: &'static str,
    value: u32,
    mutate: impl FnOnce(&mut DeviceEvidenceSnapshot),
) {
    let mut malformed = snapshot(42);
    mutate(&mut malformed);
    assert!(matches!(
        try_encode_device_snapshot(
            malformed.clone(),
            executor_snapshot(),
            host_snapshot(),
            crate::ProgramEvidenceSnapshot::NoProgram,
        ),
        Err(GateError::NonCanonicalDpcCounter {
            register: observed_register,
            value: observed_value,
        }) if observed_register == register && observed_value == value
    ));

    let mut gate = FixedCycleDigestGate::new(42);
    assert!(matches!(
        gate.capture_device_snapshot(
            malformed,
            executor_snapshot(),
            host_snapshot(),
            crate::ProgramEvidenceSnapshot::NoProgram,
        ),
        Err(GateError::NonCanonicalDpcCounter {
            register: observed_register,
            value: observed_value,
        }) if observed_register == register && observed_value == value
    ));
}

mod part1;
mod part2;
mod part3;
mod part4;
