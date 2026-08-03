use super::*;

thread_local! {
    static BOUNDARY_RDRAM: std::cell::RefCell<Box<[u8]>> =
        std::cell::RefCell::new(vec![0; rdram_len()].into_boxed_slice());
}

struct BoundaryRenderBackend;

impl fn64_render::RenderBackend for BoundaryRenderBackend {
    fn create(
        &mut self,
        _cfg: &fn64_render::RenderConfig,
    ) -> Result<(), fn64_render::RenderError> {
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

fn boundary_rdram() -> (*mut u8, usize) {
    BOUNDARY_RDRAM.with(|cell| {
        let mut storage = cell.borrow_mut();
        (storage.as_mut_ptr(), storage.len())
    })
}

fn install_boundary_render_backend() {
    let (rdram, rdram_len) = boundary_rdram();
    // SAFETY: BOUNDARY_RDRAM owns a fixed-size boxed allocation for the
    // lifetime of this test thread. The allocation is never resized or
    // replaced, and boot_thread0 tests below reuse this exact pointer.
    unsafe { fn64_abi::register_process_rdram(rdram, rdram_len) };
    fn64_abi::set_render_backend(Box::new(BoundaryRenderBackend), rdram_len);
}

fn commit_synthetic_boundary(cycle: u64) -> Result<CommittedViBoundary, ViBoundaryError> {
    install_boundary_render_backend();
    commit_scheduled_vi_boundary_with_program(cycle, ReleaseProgramDescriptor::NoProgram)
}

#[test]
fn rdram_length_covers_physical_memory_and_raw_mmio_window() {
    assert!(rdram_len() >= DEFAULT_RDRAM_SIZE);
    assert!(rdram_len() >= fn64_runtime::RDRAM_MMIO_WINDOW_END as usize);
}

#[test]
fn native_program_identity_parser_is_exact_and_lowercase() {
    let value = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let identity = NativeProgramArtifactIdentity::from_hex(value).unwrap();
    assert_eq!(
        identity.bytes()[0..8],
        [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]
    );
    assert!(matches!(
        NativeProgramArtifactIdentity::from_hex("00"),
        Err(NativeProgramIdentityError::WrongLength(2))
    ));
    let uppercase = "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef";
    assert!(matches!(
        NativeProgramArtifactIdentity::from_hex(uppercase),
        Err(NativeProgramIdentityError::InvalidHex { index: 10 })
    ));
}

#[test]
fn television_standard_is_explicit_boot_state_not_zero_fill_accident() {
    for (tv_type, expected) in [(TvType::Pal, 0), (TvType::Ntsc, 1), (TvType::Mpal, 2)] {
        let rdram = new_rdram(tv_type);
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(view.read_u32(fn64_runtime::OS_TV_TYPE_ADDR), expected);
        assert_eq!(
            view.read_u32(fn64_runtime::OS_ROM_BASE_ADDR),
            fn64_runtime::CART_ROM_KSEG1_BASE
        );
        assert_eq!(view.read_u32(fn64_runtime::OS_RESET_TYPE_ADDR), 0);
        assert_eq!(fn64_abi::configured_tv_type(), tv_type);
        assert_eq!(
            fn64_abi::vi_field_interval(),
            Some(tv_type.nominal_field_cycles())
        );
    }
}

#[test]
fn ipl3_image_seeding_uses_the_public_rom_and_rdram_ranges() {
    let mut rom = vec![0u8; 0x10_1000];
    rom[0x1000] = 0x12;
    rom[0x10_0fff] = 0x34;
    let mut rdram = vec![0u8; 0x10_0400];

    seed_ipl3_image(&mut rdram, &rom);

    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert_eq!(
        view.read_u8(fn64_runtime::RdramAddr::from_offset(0x400)),
        0x12
    );
    assert_eq!(
        view.read_u8(fn64_runtime::RdramAddr::from_offset(0x10_03ff)),
        0x34
    );
}

#[test]
fn resident_section_seeding_obeys_registered_geometry() {
    let mut rom = vec![0u8; 0x40];
    rom[0x20..0x24].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
    let mut rdram = vec![0u8; 0x80];

    seed_resident_sections(&mut rdram, &rom, &[(0x20, 0x8000_0040, 4)]);

    let mut actual = [0u8; 4];
    fn64_runtime::RdramView::from_storage(&rdram)
        .copy_logical_bytes(fn64_runtime::RdramAddr::from_offset(0x40), &mut actual);
    assert_eq!(actual, [0x11, 0x22, 0x33, 0x44]);
}

#[test]
fn guest_drain_uses_idle_quiescence_not_a_resume_count() {
    let mut drain = GuestDrain::default();

    for _ in 0..250 {
        assert_eq!(drain.before_step(Some(10)), DrainDecision::Step);
        drain.record_step(10);
    }
    assert_eq!(drain.before_step(Some(0)), DrainDecision::Step);
    drain.record_step(0);
    assert_eq!(drain.before_step(Some(0)), DrainDecision::AdvanceField);

    drain.begin_field();
    assert_eq!(drain.before_step(Some(0)), DrainDecision::Step);
    assert_eq!(drain.before_step(None), DrainDecision::AdvanceField);
}

#[test]
fn guest_drain_observes_the_authoritative_vi_deadline() {
    fn64_abi::load_rom(Vec::new());
    fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
    install_boundary_render_backend();
    let scheduled = fn64_abi::next_vi_deadline().expect("VI configured");
    let mut drain = GuestDrain::default();

    assert_eq!(
        drain.advance_to_next_device_event(),
        DeviceAdvance::ViFields {
            retrace_ticks: std::num::NonZeroU32::new(1).unwrap(),
        }
    );
    assert!(fn64_abi::next_vi_deadline().is_some_and(|next| next > scheduled));
}

#[test]
fn guest_drain_catches_up_every_overdue_vi_deadline() {
    fn64_abi::load_rom(Vec::new());
    fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
    install_boundary_render_backend();
    let first = fn64_abi::next_vi_deadline().expect("VI configured");
    let interval = fn64_abi::vi_field_interval().expect("VI interval configured");
    let current = first + interval * 2 + 1;
    let mut context = fn64_abi::RecompContext::zeroed();
    context.r4 = current >> 32;
    context.r5 = current & u64::from(u32::MAX);
    // SAFETY: osSetTime reads only the integer argument pair and ignores
    // RDRAM. Moving executor time ahead of the fabric reproduces the
    // translated-checkpoint catch-up shape this host helper must accept.
    unsafe { fn64_abi::osSetTime_recomp(std::ptr::null_mut(), &mut context) };

    let mut drain = GuestDrain::default();
    assert_eq!(
        drain.advance_to_next_device_event(),
        DeviceAdvance::ViFields {
            retrace_ticks: std::num::NonZeroU32::new(3).unwrap(),
        }
    );
    assert!(fn64_abi::next_vi_deadline().is_some_and(|next| next > current));
}

#[test]
fn quiescent_discovery_parses_conflicts_and_requires_a_real_boundary() {
    assert_eq!(parse_quiescent_discovery(None, None, false).unwrap(), None);
    assert_eq!(
        parse_quiescent_discovery(Some("nope"), None, false),
        Err(QuiescentDiscoveryError::InvalidFloor("nope".to_owned()))
    );
    assert_eq!(
        parse_quiescent_discovery(Some("10"), Some("20"), false),
        Err(QuiescentDiscoveryError::ConflictsWithReleaseGate)
    );
    assert_eq!(
        parse_quiescent_discovery(Some("10"), None, true),
        Err(QuiescentDiscoveryError::ConflictsWithReleaseGate)
    );

    let discovery = parse_quiescent_discovery(Some("10"), None, false)
        .unwrap()
        .unwrap();
    assert!(!discovery.matches(DrainDecision::AdvanceField, 9));
    assert!(!discovery.matches(DrainDecision::Step, 10));
    assert!(discovery.matches(DrainDecision::AdvanceField, 10));
    assert!(discovery.matches(DrainDecision::AdvanceField, 11));
}

#[test]
fn presentation_boundary_requires_host_advance_and_exact_capture_cycle() {
    let boundary = PresentationReleaseBoundary::new(20);
    assert!(!boundary.matches(ReleaseCycleArrival::InstructionCheckpoint, 20));
    assert!(!boundary.matches(ReleaseCycleArrival::HostAdvanceCommitted, 19));
    assert!(boundary.matches(ReleaseCycleArrival::HostAdvanceCommitted, 20));

    assert_eq!(
        parse_presentation_discovery(Some("bad"), false, None, false),
        Err(PresentationDiscoveryError::InvalidFloor("bad".to_owned()))
    );
    assert_eq!(
        parse_presentation_discovery(Some("10"), true, None, false),
        Err(PresentationDiscoveryError::ConflictsWithReleaseMode)
    );
    let discovery = parse_presentation_discovery(Some("10"), false, None, false)
        .unwrap()
        .unwrap();
    assert!(!discovery.matches(ReleaseCycleArrival::InstructionCheckpoint, 10, 10));
    assert!(!discovery.matches(ReleaseCycleArrival::HostAdvanceCommitted, 9, 9));
    assert!(!discovery.matches(ReleaseCycleArrival::HostAdvanceCommitted, 10, 9));
    assert!(discovery.matches(ReleaseCycleArrival::HostAdvanceCommitted, 10, 10));

    assert_eq!(select_release_vi_edge(10, 20, None), Ok(20));
    assert_eq!(select_release_vi_edge(10, 20, Some(30)), Ok(20));
    assert_eq!(select_release_vi_edge(10, 20, Some(20)), Ok(20));
    assert_eq!(
        select_release_vi_edge(10, 10, None),
        Err(ReleaseViEdgeError::NonMonotonic {
            current: 10,
            next_vi: 10
        })
    );
    assert_eq!(
        select_release_vi_edge(10, 20, Some(19)),
        Err(ReleaseViEdgeError::GateBeforeNextVi {
            gate: 19,
            next_vi: 20
        })
    );
}

#[test]
fn committed_vi_boundary_is_exact_and_expires_after_further_execution() {
    unsafe extern "C" fn return_immediately(
        _rdram: *mut u8,
        _ctx: *mut fn64_abi::RecompContext,
    ) {
    }

    fn64_abi::load_rom(Vec::new());
    fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
    let scheduled = fn64_abi::next_vi_deadline().unwrap();
    assert!(matches!(
        commit_scheduled_vi_boundary_with_program(
            scheduled - 1,
            ReleaseProgramDescriptor::NoProgram,
        ),
        Err(ViBoundaryError::WrongScheduledCycle { .. })
    ));

    let boundary = commit_synthetic_boundary(scheduled).unwrap();
    assert_eq!(boundary.cycle(), scheduled);
    assert_eq!(boundary.validate_unconsumed(), Ok(()));
    fn64_abi::set_trace_enabled(false);
    let (rdram, rdram_len) = boundary_rdram();
    unsafe {
        fn64_abi::boot_thread0(rdram, rdram_len, return_immediately, 99, 10);
    }
    assert!(fn64_abi::run_one_step());
    assert_eq!(fn64_abi::sim_time(), scheduled);
    assert_eq!(
        boundary.validate_unconsumed(),
        Err(ViBoundaryError::GuestStateAdvanced)
    );
}

#[test]
fn committed_vi_boundary_freezes_runtime_evidence_at_the_edge() {
    fn64_abi::load_rom(Vec::new());
    fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
    let scheduled = fn64_abi::next_vi_deadline().unwrap();
    let boundary = commit_synthetic_boundary(scheduled).unwrap();
    let edge_device = boundary.device_snapshot.clone();
    let edge_executor = boundary.executor_snapshot.clone();
    let edge_host = boundary.host_snapshot.clone();
    let edge_peripherals = edge_host.runtime_peripherals.clone();

    fn64_abi::set_controller_port_state(
        0,
        fn64_runtime::PortState::StandardControllerRumblePak,
    );
    fn64_abi::set_controller_state(0, 0xa55a, -37, 63);
    let black = edge_peripherals
        .peripherals
        .vi
        .next_blanked
        .is_none_or(|queued| !queued);
    // An all-zero context is a valid integer-only ABI call frame; this
    // shim reads only r4 and ignores the RDRAM pointer.
    let mut context: fn64_abi::RecompContext = unsafe { std::mem::zeroed() };
    context.r4 = u64::from(black);
    unsafe {
        fn64_abi::osViBlack_recomp(std::ptr::null_mut(), &mut context);
    }

    assert_ne!(fn64_abi::peripherals_evidence_snapshot(), edge_peripherals);
    assert_eq!(boundary.validate_unconsumed(), Ok(()));
    let (
        captured_device,
        captured_executor,
        captured_host,
        _captured_program,
        _captured_destinations,
        _captured_rsp_rdp,
        _captured_platform,
        _captured_windows_version,
        _captured_renderer,
        _captured_fixed_cycle,
    ) = boundary.into_evidence().unwrap();
    assert_eq!(captured_device, edge_device);
    assert_eq!(captured_executor, edge_executor);
    assert_eq!(captured_host, edge_host);
}

#[test]
fn committed_vi_boundary_owns_memory_and_audio_before_post_edge_host_mutation() {
    fn64_abi::load_rom(Vec::new());
    fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
    fn64_abi::set_audio_digest_capture(true);
    install_boundary_render_backend();
    BOUNDARY_RDRAM.with(|cell| {
        let mut storage = cell.borrow_mut();
        fn64_runtime::RdramViewMut::from_storage(&mut storage)
            .write_u32(fn64_runtime::RdramAddr::from_offset(0), 0x0123_4567);
    });

    let scheduled = fn64_abi::next_vi_deadline().unwrap();
    let boundary = commit_synthetic_boundary(scheduled).unwrap();
    assert_eq!(
        &boundary
            .fixed_cycle
            .physical_rdram_logical
            .as_ref()
            .unwrap()[..4],
        &[0x01, 0x23, 0x45, 0x67]
    );
    assert_eq!(boundary.fixed_cycle.audio_pcm_s16le, Some(Vec::new()));

    BOUNDARY_RDRAM.with(|cell| {
        let mut storage = cell.borrow_mut();
        fn64_runtime::RdramViewMut::from_storage(&mut storage)
            .write_u32(fn64_runtime::RdramAddr::from_offset(0), 0x89ab_cdef);
    });
    fn64_abi::set_audio_digest_capture(false);

    // Raw host writes and capture-control changes are not guest execution;
    // the boundary remains consumable and retains its edge-owned bytes.
    assert_eq!(boundary.validate_unconsumed(), Ok(()));
    let (_, _, _, _, _, _, _, _, _, fixed_cycle) = boundary.into_evidence().unwrap();
    assert_eq!(
        &fixed_cycle.physical_rdram_logical.unwrap()[..4],
        &[0x01, 0x23, 0x45, 0x67]
    );
    assert_eq!(fixed_cycle.audio_pcm_s16le, Some(Vec::new()));
}

#[test]
fn committed_vi_boundary_expires_after_a_controller_operation() {
    fn64_abi::load_rom(Vec::new());
    fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
    fn64_abi::set_controller_port_state(0, fn64_runtime::PortState::StandardControllerNoPak);
    let scheduled = fn64_abi::next_vi_deadline().unwrap();
    let boundary = commit_synthetic_boundary(scheduled).unwrap();

    let operations_before = fn64_abi::copy_controller_operations().len();
    let mut rdram = vec![0u8; 64];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_u8(fn64_runtime::RdramAddr::from_offset(0), 1);
        view.write_u8(fn64_runtime::RdramAddr::from_offset(1), 4);
        view.write_u8(fn64_runtime::RdramAddr::from_offset(2), 0x01);
        view.write_u8(fn64_runtime::RdramAddr::from_offset(7), 0xFE);
    }
    let mut context: fn64_abi::RecompContext = unsafe { std::mem::zeroed() };
    context.r4 = 1;
    context.r5 = 0x8000_0000;
    unsafe {
        fn64_abi::__osSiRawStartDma_recomp(rdram.as_mut_ptr(), &mut context);
    }
    assert_eq!(context.r2, 0);
    fn64_abi::advance_virtual_time(fn64_abi::next_device_deadline().unwrap());
    assert_eq!(
        fn64_abi::copy_controller_operations().len(),
        operations_before + 1
    );

    assert_eq!(
        boundary.validate_unconsumed(),
        Err(ViBoundaryError::GuestStateAdvanced)
    );
}

#[test]
fn committed_vi_boundary_expires_after_native_destination_entry() {
    unsafe extern "C" fn entered_after_boundary(
        _rdram: *mut u8,
        _ctx: *mut fn64_abi::RecompContext,
    ) {
    }

    fn64_abi::load_rom(Vec::new());
    fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
    let scheduled = fn64_abi::next_vi_deadline().unwrap();
    let boundary = commit_synthetic_boundary(scheduled).unwrap();
    unsafe {
        fn64_abi::register_section(
            0x0010_0000,
            0x8000_2000,
            4,
            &[(0, 4, entered_after_boundary)],
        );
    }
    fn64_abi::fn64_c_recompiled_function_enter(entered_after_boundary);

    assert_eq!(
        boundary.validate_unconsumed(),
        Err(ViBoundaryError::GuestStateAdvanced)
    );
}

#[cfg(feature = "recomp-rs")]
fn observed_function_lookup(_vram: u32) -> fn64_recomp_rs::RecompFunc {
    fn observed_function(
        _ctx: &mut fn64_recomp_rs::RecompContext,
        _rdram: &mut fn64_recomp_rs::Rdram<'_>,
    ) {
    }
    observed_function
}

#[cfg(feature = "recomp-rs")]
#[test]
fn committed_vi_boundary_freezes_observed_function_destinations() {
    std::thread::spawn(|| {
        use fn64_recomp_rs::{
            ProgramArtifactIdentity, TranslatedFunctionIdentity,
            FUNCTION_ENTRY_OBSERVATION_SCHEMA,
        };

        fn64_abi::load_rom(Vec::new());
        fn64_abi::recompiled::set_entry_lookup_with_execution_observation(
            observed_function_lookup,
            0x100,
            ProgramArtifactIdentity::new([0x5a; 32]),
            FUNCTION_ENTRY_OBSERVATION_SCHEMA,
        );
        fn64_recomp_rs::notify_function_entry(TranslatedFunctionIdentity::new(
            0x8000_1000,
            "entry",
        ));
        fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
        let scheduled = fn64_abi::next_vi_deadline().unwrap();
        install_boundary_render_backend();
        let boundary = commit_scheduled_vi_boundary(scheduled).unwrap();
        assert_eq!(boundary.function_execution_destinations.len(), 1);
        assert_eq!(boundary.validate_unconsumed(), Ok(()));

        fn64_recomp_rs::notify_function_entry(TranslatedFunctionIdentity::new(
            0x8000_2000,
            "callee",
        ));
        assert_eq!(
            boundary.validate_unconsumed(),
            Err(ViBoundaryError::GuestStateAdvanced)
        );
    })
    .join()
    .unwrap();
}

#[cfg(feature = "recomp-rs")]
#[test]
fn committed_vi_boundary_rejects_identity_only_function_lane() {
    std::thread::spawn(|| {
        fn64_abi::load_rom(Vec::new());
        fn64_abi::recompiled::set_entry_lookup_with_artifact_identity(
            observed_function_lookup,
            0x100,
            fn64_recomp_rs::ProgramArtifactIdentity::new([0x5b; 32]),
        );
        fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
        let scheduled = fn64_abi::next_vi_deadline().unwrap();
        install_boundary_render_backend();
        let failure = std::panic::catch_unwind(|| commit_scheduled_vi_boundary(scheduled))
            .expect_err("identity-only function lane must fail the observation-schema gate");
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(message.contains("entry-observation schema"));
    })
    .join()
    .unwrap();
}

#[test]
fn live_gate_rejects_expired_boundary_without_writing_a_report() {
    unsafe extern "C" fn return_immediately(
        _rdram: *mut u8,
        _ctx: *mut fn64_abi::RecompContext,
    ) {
    }

    fn64_abi::load_rom(Vec::new());
    fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
    let scheduled = fn64_abi::next_vi_deadline().unwrap();
    let mut gate = LiveReleaseGate::new(scheduled);
    gate.arm().unwrap();
    fn64_abi::set_trace_enabled(false);
    let boundary = commit_synthetic_boundary(scheduled).unwrap();

    let (rdram, rdram_len) = boundary_rdram();
    unsafe {
        fn64_abi::boot_thread0(rdram, rdram_len, return_immediately, 100, 10);
    }
    assert!(fn64_abi::run_one_step());
    assert_eq!(fn64_abi::sim_time(), scheduled);

    let path = std::env::temp_dir().join(format!(
        "fn64-expired-boundary-{}-{scheduled}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let result = gate.capture_and_write_observed(
        boundary,
        "expired-boundary",
        b"input",
        None,
        release_gate::LiveObservedArtifacts {
            framebuffer_artifact_bytes: b"framebuffer",
            framebuffer_payload_bytes: 2,
            observations: ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap(),
        },
        &path,
    );
    assert!(matches!(
        result,
        Err(GateError::InvalidViBoundary(
            ViBoundaryError::GuestStateAdvanced
        ))
    ));
    assert!(!path.exists());
}

#[test]
fn live_gate_rejects_legacy_unidentified_native_boundary() {
    fn64_abi::load_rom(Vec::new());
    fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
    let scheduled = fn64_abi::next_vi_deadline().unwrap();
    let mut gate = LiveReleaseGate::new(scheduled);
    gate.arm().unwrap();
    install_boundary_render_backend();
    let boundary = commit_scheduled_vi_boundary(scheduled).unwrap();
    let path = std::env::temp_dir().join(format!(
        "fn64-unidentified-native-{}-{scheduled}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let result = gate.capture_and_write_observed(
        boundary,
        "unidentified-native",
        b"input",
        None,
        release_gate::LiveObservedArtifacts {
            framebuffer_artifact_bytes: b"framebuffer",
            framebuffer_payload_bytes: 2,
            observations: ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap(),
        },
        &path,
    );
    assert!(matches!(result, Err(GateError::UnidentifiedNativeProgram)));
    assert!(!path.exists());
}

#[cfg(unix)]
#[test]
fn release_env_presence_never_silently_discards_non_unicode_values() {
    use std::os::unix::ffi::OsStringExt as _;

    assert_eq!(
        parse_release_env_value("MODE", Some(std::ffi::OsString::from("10"))).unwrap(),
        Some("10".to_owned())
    );
    assert_eq!(parse_release_env_value("MODE", None).unwrap(), None);
    assert_eq!(
        parse_release_env_value("MODE", Some(std::ffi::OsString::from_vec(vec![0xff]))),
        Err(ReleaseEnvError { name: "MODE" })
    );
}
