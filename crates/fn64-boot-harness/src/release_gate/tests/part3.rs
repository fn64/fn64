use super::*;

#[test]
fn digest_integrity_rejects_stale_root_and_noncanonical_artifact_sets() {
    let valid = complete_digest();

    let mut stale_root = valid.clone();
    stale_root.root_sha256 =
        "f61d68656ce63b773664e4bdf7b19017697cda2232c10f01ca7bde3a9f910705".to_owned();
    assert!(matches!(
        stale_root.verify_integrity(),
        Err(GateError::DigestRootIntegrityMismatch { .. })
    ));

    let mut missing = valid.clone();
    missing.artifacts.pop();
    assert!(matches!(
        missing.verify_integrity(),
        Err(GateError::InvalidArtifactSet { .. })
    ));

    let mut duplicate = valid.clone();
    duplicate.artifacts[1].kind = ArtifactKind::Framebuffer;
    assert!(matches!(
        duplicate.verify_integrity(),
        Err(GateError::InvalidArtifactSet { .. })
    ));

    let mut reordered = valid;
    reordered.artifacts.swap(0, 1);
    assert!(matches!(
        reordered.verify_integrity(),
        Err(GateError::InvalidArtifactSet { .. })
    ));

    let mut noncanonical_sha = complete_digest();
    noncanonical_sha.artifacts[0].sha256.make_ascii_uppercase();
    assert!(matches!(
        noncanonical_sha.verify_integrity(),
        Err(GateError::InvalidReportSha256("digest.artifacts[].sha256"))
    ));
}

#[test]
fn report_rejects_artifact_counts_that_disagree_with_observations() {
    let make_report = |digest| {
        ReleaseGateReport::new(
            "count-binding",
            b"input",
            digest,
            observations(),
            Vec::new(),
        )
    };

    let mut wrong_memory = complete_digest();
    wrong_memory.artifacts[2].bytes -= 1;
    wrong_memory.root_sha256 =
        recompute_digest_root(wrong_memory.guest_cycle, &wrong_memory.artifacts).unwrap();
    assert!(matches!(
        make_report(wrong_memory),
        Err(GateError::ArtifactObservationByteMismatch {
            kind: ArtifactKind::Memory,
            ..
        })
    ));

    let mut wrong_reference_framebuffer = complete_digest();
    wrong_reference_framebuffer.artifacts[0].bytes += 1;
    wrong_reference_framebuffer.root_sha256 = recompute_digest_root(
        wrong_reference_framebuffer.guest_cycle,
        &wrong_reference_framebuffer.artifacts,
    )
    .unwrap();
    assert!(matches!(
        make_report(wrong_reference_framebuffer),
        Err(GateError::ArtifactObservationByteMismatch {
            kind: ArtifactKind::Framebuffer,
            ..
        })
    ));

    let geometry = ReleaseObservationGeometry::post_vi_swapchain(
        authoritative_rt64_identity(),
        "11".repeat(32),
        1,
        1,
        1,
        1,
        4,
        4,
    )
    .unwrap();
    let expected = geometry.expected_framebuffer_artifact_bytes().unwrap();
    let mut digest = complete_digest();
    digest.artifacts[0].bytes = expected - 1;
    digest.root_sha256 = recompute_digest_root(digest.guest_cycle, &digest.artifacts).unwrap();
    assert!(matches!(
        ReleaseGateReport::new("post-vi-count", b"input", digest, geometry, Vec::new()),
        Err(GateError::ArtifactObservationByteMismatch {
            kind: ArtifactKind::Framebuffer,
            ..
        })
    ));
}

#[test]
fn report_rejects_contradictory_closure_states() {
    let base = ClosurePath {
        name: "path".to_owned(),
        observations: 1,
        status: ClosurePathStatus::ExercisedZeroUnsupported,
        unsupported: Vec::new(),
    };
    let event = UnsupportedEvent {
        subsystem: "test".to_owned(),
        operation: "unsupported".to_owned(),
        context: "fixture".to_owned(),
        guest_cycle: Some(42),
        disposition: "loud_trap".to_owned(),
    };
    let mut invalid = Vec::new();
    invalid.push(ClosurePath {
        observations: 1,
        status: ClosurePathStatus::Unexercised,
        ..base.clone()
    });
    invalid.push(ClosurePath {
        observations: 0,
        ..base.clone()
    });
    invalid.push(ClosurePath {
        status: ClosurePathStatus::ExercisedUnsupported,
        ..base.clone()
    });
    invalid.push(ClosurePath {
        observations: 1,
        status: ClosurePathStatus::ExercisedUnsupported,
        unsupported: vec![event.clone(), event],
        ..base
    });

    for closure in invalid {
        assert!(matches!(
            ReleaseGateReport::new(
                "closure-invariant",
                b"input",
                complete_digest(),
                observations(),
                vec![closure],
            ),
            Err(GateError::InvalidClosurePath { .. })
        ));
    }
}

#[test]
fn report_sha_binds_input_scenario_digest_and_canonical_closure() {
    let paths = vec![
        ClosurePath {
            name: "render".to_owned(),
            observations: 2,
            status: ClosurePathStatus::ExercisedZeroUnsupported,
            unsupported: Vec::new(),
        },
        ClosurePath {
            name: "cpu".to_owned(),
            observations: 1,
            status: ClosurePathStatus::ExercisedZeroUnsupported,
            unsupported: Vec::new(),
        },
    ];
    let report = ReleaseGateReport::new(
        "rs-reference-lle",
        b"rom-a",
        complete_digest(),
        observations(),
        paths.clone(),
    )
    .unwrap();
    let reordered = ReleaseGateReport::new(
        "rs-reference-lle",
        b"rom-a",
        complete_digest(),
        observations(),
        paths.into_iter().rev().collect(),
    )
    .unwrap();
    assert_eq!(report.report_sha256, reordered.report_sha256);

    let different_scenario = ReleaseGateReport::new(
        "rs-rt64-lle",
        b"rom-a",
        complete_digest(),
        observations(),
        report.closure.clone(),
    )
    .unwrap();
    let different_input = ReleaseGateReport::new(
        "rs-reference-lle",
        b"rom-b",
        complete_digest(),
        observations(),
        report.closure.clone(),
    )
    .unwrap();
    assert_ne!(report.report_sha256, different_scenario.report_sha256);
    assert_ne!(report.report_sha256, different_input.report_sha256);
    report.verify_integrity().unwrap();

    let mut duplicate_closure = report.clone();
    duplicate_closure
        .closure
        .push(duplicate_closure.closure[0].clone());
    duplicate_closure.report_sha256 =
        sha256_hex(&encode_report_evidence(&duplicate_closure).unwrap());
    assert!(matches!(
        duplicate_closure.verify_integrity(),
        Err(GateError::DuplicateClosurePath(_))
    ));

    let mut empty_closure_name = report.clone();
    empty_closure_name.closure[0].name.clear();
    empty_closure_name.report_sha256 =
        sha256_hex(&encode_report_evidence(&empty_closure_name).unwrap());
    assert!(matches!(
        empty_closure_name.verify_integrity(),
        Err(GateError::EmptyPathName)
    ));

    let mut reordered_closure = report.clone();
    reordered_closure.closure.swap(0, 1);
    reordered_closure.report_sha256 =
        sha256_hex(&encode_report_evidence(&reordered_closure).unwrap());
    assert!(matches!(
        reordered_closure.verify_integrity(),
        Err(GateError::NonCanonicalClosureOrder { .. })
    ));

    let mut relabeled_source = report.clone();
    let FramebufferObservationSource::PhysicalRdram { address } =
        &mut relabeled_source.observations.framebuffer.source
    else {
        unreachable!("test report uses physical RDRAM")
    };
    *address = 2;
    assert!(matches!(
        relabeled_source.verify_integrity(),
        Err(GateError::ReportIntegrityMismatch { .. })
    ));

    let encoded = serde_json::to_vec(&report).unwrap();
    let mut retained: ReleaseGateReport = serde_json::from_slice(&encoded).unwrap();
    retained.scenario.push_str("-mutated");
    assert!(matches!(
        retained.verify_integrity(),
        Err(GateError::ReportIntegrityMismatch { .. })
    ));
    assert!(matches!(
        retained.require_closed(),
        Err(GateError::ReportIntegrityMismatch { .. })
    ));

    let mut changed_instrumentation = report.clone();
    changed_instrumentation.unsupported_instrumentation.sha256 = "00".repeat(32);
    assert!(matches!(
        changed_instrumentation.verify_integrity(),
        Err(GateError::UnsupportedInstrumentationIdentityMismatch { .. })
    ));

    let mut stale_schema = report.clone();
    stale_schema.schema = "fn64.release-gate.v28".to_owned();
    assert!(matches!(
        stale_schema.verify_integrity(),
        Err(GateError::UnsupportedReportSchema(schema))
            if schema == "fn64.release-gate.v28"
    ));

    let duplicate = vec![report.closure[0].clone(), report.closure[0].clone()];
    assert!(matches!(
        ReleaseGateReport::new(
            "duplicate",
            b"rom",
            complete_digest(),
            observations(),
            duplicate,
        ),
        Err(GateError::DuplicateClosurePath(_))
    ));
}

#[test]
fn schema_v29_report_wire_binds_every_release_environment_field() {
    let report = ReleaseGateReport::new(
        "environment-wire",
        b"input",
        complete_digest(),
        observations(),
        Vec::new(),
    )
    .unwrap();
    let digest = |value: &ReleaseGateReport| {
        sha256_hex(&encode_report_evidence(value).expect("environment encodes"))
    };
    let baseline = digest(&report);

    for platform in [
        ReleaseHostPlatform::MacosArm64,
        ReleaseHostPlatform::LinuxX86_64,
        ReleaseHostPlatform::WindowsX86_64,
    ] {
        if platform != report.environment.platform {
            let mut changed = report.clone();
            changed.environment.platform = platform;
            assert_ne!(digest(&changed), baseline, "platform tag collided");
        }
    }

    let mut windows = report.clone();
    windows.environment.platform = ReleaseHostPlatform::WindowsX86_64;
    windows.environment.windows_version = Some(
        ReleaseWindowsVersionEvidence::from_native_workstation(10, 0, 22_000, 123).unwrap(),
    );
    validate_environment_evidence(&windows.environment).unwrap();
    let windows_baseline = digest(&windows);
    for mutate in [
        |version: &mut ReleaseWindowsVersionEvidence| version.major = 11,
        |version: &mut ReleaseWindowsVersionEvidence| version.minor = 1,
        |version: &mut ReleaseWindowsVersionEvidence| version.build += 1,
        |version: &mut ReleaseWindowsVersionEvidence| version.update_build_revision += 1,
        |version: &mut ReleaseWindowsVersionEvidence| {
            version.family = ReleaseWindowsFamily::Windows10
        },
    ] {
        let mut changed = windows.clone();
        mutate(changed.environment.windows_version.as_mut().unwrap());
        assert_ne!(
            digest(&changed),
            windows_baseline,
            "Windows identity field collided"
        );
    }
    let mut missing_windows_version = windows.clone();
    missing_windows_version.environment.windows_version = None;
    assert!(matches!(
        validate_environment_evidence(&missing_windows_version.environment),
        Err(GateError::InvalidWindowsVersionEvidence(_))
    ));
    let mut attached_to_macos = windows;
    attached_to_macos.environment.platform = ReleaseHostPlatform::MacosArm64;
    assert!(matches!(
        validate_environment_evidence(&attached_to_macos.environment),
        Err(GateError::InvalidWindowsVersionEvidence(_))
    ));

    let port_states = [
        ReleaseControllerPort::StandardControllerNoPak,
        ReleaseControllerPort::StandardControllerControllerPak,
        ReleaseControllerPort::StandardControllerRumblePak,
        ReleaseControllerPort::StandardControllerTransferPak,
        ReleaseControllerPort::VoiceRecognitionUnit,
        ReleaseControllerPort::Absent,
    ];
    for index in 0..4 {
        for state in port_states {
            if state != report.environment.controller_ports[index] {
                let mut changed = report.clone();
                changed.environment.controller_ports[index] = state;
                assert_ne!(
                    digest(&changed),
                    baseline,
                    "controller port {index} state {state:?} collided"
                );
            }
        }
    }

    for save in [
        ReleaseCartridgeSave::NoCartridgeSave,
        ReleaseCartridgeSave::Eeprom4k,
        ReleaseCartridgeSave::Eeprom16k,
        ReleaseCartridgeSave::Sram32Kib,
        ReleaseCartridgeSave::FlashRam128Kib,
    ] {
        if save != report.environment.cartridge_save {
            let mut changed = report.clone();
            changed.environment.cartridge_save = save;
            assert_ne!(digest(&changed), baseline, "cartridge save tag collided");
        }
    }

    let mut changed_policy = report.clone();
    changed_policy.environment.renderer = ReleaseRendererEvidence::Reference {
        execution_policy: ReleaseGraphicsExecutionPolicy::HleOptimized,
        tv_type: ReleaseTvStandard::Ntsc,
    };
    assert_ne!(digest(&changed_policy), baseline, "render policy collided");

    let mut changed_tv = report.clone();
    let ReleaseRendererEvidence::Reference { tv_type, .. } =
        &mut changed_tv.environment.renderer
    else {
        unreachable!()
    };
    *tv_type = ReleaseTvStandard::Pal;
    assert_ne!(digest(&changed_tv), baseline, "renderer TV type collided");

    let mut rt64 = report.clone();
    rt64.environment.renderer = ReleaseRendererEvidence::Rt64 {
        execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
        tv_type: ReleaseTvStandard::Ntsc,
        graphics_api: current_test_graphics_api(),
        backend_identity: authoritative_rt64_identity(),
        source_authoritative: true,
        settings_sha256: "11".repeat(32),
        replacement_packs_active: false,
    };
    let rt64_baseline = digest(&rt64);
    assert_ne!(rt64_baseline, baseline, "renderer class collided");

    let mut changed = rt64.clone();
    let ReleaseRendererEvidence::Rt64 {
        execution_policy, ..
    } = &mut changed.environment.renderer
    else {
        unreachable!()
    };
    *execution_policy = ReleaseGraphicsExecutionPolicy::HleOptimized;
    assert_ne!(digest(&changed), rt64_baseline, "RT64 policy collided");

    let mut changed = rt64.clone();
    let ReleaseRendererEvidence::Rt64 { graphics_api, .. } = &mut changed.environment.renderer
    else {
        unreachable!()
    };
    *graphics_api = match *graphics_api {
        ReleaseGraphicsApi::D3d12 => ReleaseGraphicsApi::Vulkan,
        ReleaseGraphicsApi::Vulkan | ReleaseGraphicsApi::Metal => ReleaseGraphicsApi::D3d12,
    };
    assert_ne!(digest(&changed), rt64_baseline, "graphics API collided");

    let mut changed = rt64.clone();
    let ReleaseRendererEvidence::Rt64 {
        backend_identity, ..
    } = &mut changed.environment.renderer
    else {
        unreachable!()
    };
    backend_identity.push_str("-changed");
    assert_ne!(digest(&changed), rt64_baseline, "backend identity collided");

    let mut changed = rt64.clone();
    let ReleaseRendererEvidence::Rt64 {
        source_authoritative,
        ..
    } = &mut changed.environment.renderer
    else {
        unreachable!()
    };
    *source_authoritative = false;
    assert_ne!(digest(&changed), rt64_baseline, "source authority collided");

    let mut changed = rt64.clone();
    let ReleaseRendererEvidence::Rt64 {
        settings_sha256, ..
    } = &mut changed.environment.renderer
    else {
        unreachable!()
    };
    *settings_sha256 = "22".repeat(32);
    assert_ne!(
        digest(&changed),
        rt64_baseline,
        "settings identity collided"
    );

    let mut changed = rt64.clone();
    let ReleaseRendererEvidence::Rt64 {
        replacement_packs_active,
        ..
    } = &mut changed.environment.renderer
    else {
        unreachable!()
    };
    *replacement_packs_active = true;
    assert_ne!(
        digest(&changed),
        rt64_baseline,
        "replacement-pack activity collided"
    );
}

#[test]
fn frozen_environment_derivation_fails_closed() {
    let platform = crate::release_host_platform().expect("supported test platform");
    let reference = || fn64_abi::RenderEnvironmentEvidenceSnapshot {
        backend: fn64_abi::RenderBackendEvidence::Reference {
            tv_type: TvType::Ntsc,
        },
        execution_policy: fn64_abi::GraphicsTaskExecutionPolicy::LleAccuracy,
    };

    let mut host = host_snapshot();
    host.cartridge_save = fn64_abi::CartridgeSaveEvidenceSnapshot::Unidentified;
    assert!(matches!(
        environment_from_frozen(platform, None, &host, reference()),
        Err(GateError::UnidentifiedCartridgeSave)
    ));

    host.cartridge_save = fn64_abi::CartridgeSaveEvidenceSnapshot::NoCartridgeSave;
    assert!(matches!(
        environment_from_frozen(
            platform,
            None,
            &host,
            fn64_abi::RenderEnvironmentEvidenceSnapshot {
                backend: fn64_abi::RenderBackendEvidence::Unidentified,
                execution_policy: fn64_abi::GraphicsTaskExecutionPolicy::LleAccuracy,
            },
        ),
        Err(GateError::UnidentifiedRenderBackend)
    ));
    assert!(matches!(
        environment_from_frozen(
            platform,
            None,
            &host,
            fn64_abi::RenderEnvironmentEvidenceSnapshot {
                backend: fn64_abi::RenderBackendEvidence::Reference {
                    tv_type: TvType::Ntsc,
                },
                execution_policy: fn64_abi::GraphicsTaskExecutionPolicy::HleOptimized,
            },
        ),
        Err(GateError::NonAccuracyRenderPolicy)
    ));
    assert!(matches!(
        environment_from_frozen(
            platform,
            None,
            &host,
            fn64_abi::RenderEnvironmentEvidenceSnapshot {
                backend: fn64_abi::RenderBackendEvidence::Reference {
                    tv_type: TvType::Ntsc,
                },
                execution_policy: fn64_abi::GraphicsTaskExecutionPolicy::DiagnosticSkip,
            },
        ),
        Err(GateError::NonAccuracyRenderPolicy)
    ));
}

#[test]
fn frozen_environment_derivation_preserves_each_concrete_graphics_api() {
    let platform = crate::release_host_platform().expect("supported test platform");
    let mut host = host_snapshot();
    host.cartridge_save = fn64_abi::CartridgeSaveEvidenceSnapshot::NoCartridgeSave;

    for (active, expected) in [
        (
            fn64_abi::ActiveRenderGraphicsApi::D3d12,
            ReleaseGraphicsApi::D3d12,
        ),
        (
            fn64_abi::ActiveRenderGraphicsApi::Vulkan,
            ReleaseGraphicsApi::Vulkan,
        ),
        (
            fn64_abi::ActiveRenderGraphicsApi::Metal,
            ReleaseGraphicsApi::Metal,
        ),
    ] {
        let environment = environment_from_frozen(
            platform,
            None,
            &host,
            fn64_abi::RenderEnvironmentEvidenceSnapshot {
                backend: fn64_abi::RenderBackendEvidence::Rt64 {
                    tv_type: TvType::Ntsc,
                    backend_identity: authoritative_rt64_identity_for(expected),
                    source_authoritative: true,
                    graphics_api: active,
                    settings_sha256: [0x11; 32],
                    replacement_packs_active: false,
                },
                execution_policy: fn64_abi::GraphicsTaskExecutionPolicy::LleAccuracy,
            },
        )
        .unwrap();
        assert!(matches!(
            environment.renderer,
            ReleaseRendererEvidence::Rt64 { graphics_api, .. } if graphics_api == expected
        ));
    }
}

#[test]
fn release_renderer_json_requires_a_concrete_rt64_api_only() {
    let rt64 = serde_json::json!({
        "kind": "rt64",
        "execution_policy": "lle_accuracy",
        "graphics_api": "automatic",
        "backend_identity": "identity",
        "source_authoritative": true,
        "settings_sha256": "11".repeat(32),
        "replacement_packs_active": false,
    });
    assert!(serde_json::from_value::<ReleaseRendererEvidence>(rt64).is_err());

    let reference_with_api = serde_json::json!({
        "kind": "reference",
        "execution_policy": "lle_accuracy",
        "graphics_api": "vulkan",
    });
    assert!(
        serde_json::from_value::<ReleaseRendererEvidence>(reference_with_api).is_err(),
        "reference evidence must reject an RT64-only graphics API field"
    );
}

#[test]
fn release_environment_rejects_cross_platform_graphics_api_pair() {
    let platform = crate::release_host_platform().expect("supported test platform");
    let valid_api = current_test_graphics_api();
    let environment = ReleaseEnvironmentEvidence {
        platform,
        windows_version: crate::test_release_windows_version(),
        controller_ports: [ReleaseControllerPort::Absent; 4],
        cartridge_save: ReleaseCartridgeSave::NoCartridgeSave,
        audio_task_execution: ReleaseAudioTaskExecutionPolicy::LleAccuracy,
        renderer: ReleaseRendererEvidence::Rt64 {
            execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
            tv_type: ReleaseTvStandard::Ntsc,
            graphics_api: valid_api,
            backend_identity: authoritative_rt64_identity_for(valid_api),
            source_authoritative: true,
            settings_sha256: "11".repeat(32),
            replacement_packs_active: false,
        },
    };
    validate_environment_evidence(&environment).unwrap();

    let invalid_api = match platform {
        ReleaseHostPlatform::MacosArm64 | ReleaseHostPlatform::LinuxX86_64 => {
            ReleaseGraphicsApi::D3d12
        }
        ReleaseHostPlatform::WindowsX86_64 => ReleaseGraphicsApi::Metal,
    };
    let mut invalid = environment;
    let ReleaseRendererEvidence::Rt64 {
        graphics_api,
        backend_identity,
        ..
    } = &mut invalid.renderer
    else {
        unreachable!()
    };
    *graphics_api = invalid_api;
    *backend_identity = authoritative_rt64_identity_for(invalid_api);
    assert!(matches!(
        validate_environment_evidence(&invalid),
        Err(GateError::RendererObservationMismatch(_))
    ));
}

#[test]
fn report_rejects_untrusted_or_mismatched_renderer_evidence() {
    let mut environment = test_release_environment(&observations());
    environment.renderer = ReleaseRendererEvidence::Rt64 {
        execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
        tv_type: ReleaseTvStandard::Ntsc,
        graphics_api: current_test_graphics_api(),
        backend_identity: "rt64-test".to_owned(),
        source_authoritative: false,
        settings_sha256: "11".repeat(32),
        replacement_packs_active: false,
    };
    assert!(matches!(
        ReleaseGateReport::new_with_test_environment(
            "untrusted-renderer",
            b"input",
            complete_digest(),
            observations(),
            environment,
            Vec::new(),
        ),
        Err(GateError::RendererObservationMismatch(_))
    ));

    let mut environment = test_release_environment(&observations());
    environment.renderer = ReleaseRendererEvidence::Rt64 {
        execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
        tv_type: ReleaseTvStandard::Ntsc,
        graphics_api: current_test_graphics_api(),
        backend_identity: "rt64-test".to_owned(),
        source_authoritative: true,
        settings_sha256: "11".repeat(32),
        replacement_packs_active: false,
    };
    assert!(matches!(
        ReleaseGateReport::new_with_test_environment(
            "mismatched-renderer",
            b"input",
            complete_digest(),
            observations(),
            environment,
            Vec::new(),
        ),
        Err(GateError::RendererObservationMismatch(_))
    ));
}

#[test]
fn timing_digest_ignores_ambient_sequence_but_rejects_future_events() {
    let event = |seq| TraceEvent {
        seq,
        sim_time: 41,
        kind: TraceKind::ThreadSwitch {
            from: Some(1),
            to: 2,
            reason: SwitchReason::PauseSelf,
        },
    };
    assert_eq!(
        encode_timing_trace(&[event(1)]),
        encode_timing_trace(&[event(9_999)])
    );

    let mut gate = FixedCycleDigestGate::new(42);
    assert!(matches!(
        gate.capture_timing_trace(
            42,
            &[TraceEvent {
                sim_time: 43,
                ..event(0)
            }],
        ),
        Err(GateError::FutureTraceEvent { .. })
    ));
}

#[test]
fn live_timing_digest_binds_typed_device_dma_and_rejects_future_events() {
    let pi = DeviceTraceEvent {
        at: fn64_runtime::EmulatedInstant::new(41),
        sequence: 500,
        kind: DeviceTraceKind::PiBytesCommitted(PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x200),
            device: PiDeviceAddress::RomOffset(0x1000),
            len: 64,
        }),
    };
    let mut left = FixedCycleDigestGate::new(42);
    left.capture_live_timing_trace(42, &[], &[pi]).unwrap();
    let left = left.artifacts[&ArtifactKind::TimingTrace].sha256.clone();

    let mut changed_sequence = pi;
    changed_sequence.sequence = 999;
    let mut right = FixedCycleDigestGate::new(42);
    right
        .capture_live_timing_trace(42, &[], &[changed_sequence])
        .unwrap();
    assert_eq!(left, right.artifacts[&ArtifactKind::TimingTrace].sha256);

    let mut changed_request = pi;
    changed_request.kind = DeviceTraceKind::PiBytesCommitted(PiDmaRequest {
        len: 128,
        ..match pi.kind {
            DeviceTraceKind::PiBytesCommitted(request) => request,
            _ => unreachable!(),
        }
    });
    let mut changed = FixedCycleDigestGate::new(42);
    changed
        .capture_live_timing_trace(42, &[], &[changed_request])
        .unwrap();
    assert_ne!(left, changed.artifacts[&ArtifactKind::TimingTrace].sha256);

    let mut future = pi;
    future.at = fn64_runtime::EmulatedInstant::new(43);
    let mut gate = FixedCycleDigestGate::new(42);
    assert!(matches!(
        gate.capture_live_timing_trace(42, &[], &[future]),
        Err(GateError::FutureDeviceTraceEvent { .. })
    ));
}

#[test]
fn wrong_cycle_and_missing_channel_fail_loudly() {
    let mut gate = FixedCycleDigestGate::new(10);
    assert!(matches!(
        gate.capture(9, ArtifactKind::Audio, b"late"),
        Err(GateError::WrongCycle { .. })
    ));
    assert!(matches!(gate.finish(), Err(GateError::MissingArtifacts(_))));
}

#[test]
fn report_distinguishes_unexercised_zero_and_unsupported() {
    let mut closure = ClosureGate::default();
    closure.declare("cpu.dynamic-target").unwrap();
    closure.declare("rsp.custom-ucode").unwrap();
    closure.declare("rdp.raw-command").unwrap();
    closure.observe_supported("rsp.custom-ucode").unwrap();
    closure
        .observe_unsupported(
            "rdp.raw-command",
            "render",
            "rdp.opcode.0x3f",
            "task=7 word=12",
            Some(42),
            "loud_trap",
        )
        .unwrap();
    let report = ReleaseGateReport::new(
        "synthetic-unsupported",
        b"synthetic input",
        complete_digest(),
        observations(),
        closure.finish(),
    )
    .unwrap();
    let error = report.require_closed().unwrap_err().to_string();
    assert!(error.contains("cpu.dynamic-target"));
    assert!(error.contains("rdp.raw-command:rdp.opcode.0x3f"));

    let json = serde_json::to_value(report).unwrap();
    assert_eq!(json["closure"][0]["status"], "unexercised");
    assert_eq!(json["closure"][1]["status"], "exercised_unsupported");
    assert_eq!(json["closure"][2]["status"], "exercised_zero_unsupported");
}

#[test]
fn live_closure_binds_typed_unsupported_source_or_proves_zero() {
    let zero = derive_live_closure(LiveClosureInputs {
        framebuffer_bytes: b"",
        audio_bytes: b"",
        memory_bytes: b"",
        trace: &[],
        device_trace: &[],
        save_operations: &[],
        controller_operations: &[],
        unsupported_events: &[],
    })
    .unwrap();
    let source = zero
        .iter()
        .find(|path| path.name == "execution.unsupported-event-source")
        .unwrap();
    assert_eq!(source.observations, 1);
    assert!(matches!(
        source.status,
        ClosurePathStatus::ExercisedZeroUnsupported
    ));

    let reached = [fn64_runtime::UnsupportedEvent {
        sequence: 99,
        subsystem: fn64_runtime::UnsupportedSubsystem::Render,
        operation: "render.hle-ucode.needs-lle".to_owned(),
        context: "microcode_sha256=0011".to_owned(),
        guest_cycle: Some(Cycles::new(42)),
        disposition: fn64_runtime::UnsupportedDisposition::NeedsLle,
    }];
    let closure = derive_live_closure(LiveClosureInputs {
        framebuffer_bytes: b"",
        audio_bytes: b"",
        memory_bytes: b"",
        trace: &[],
        device_trace: &[],
        save_operations: &[],
        controller_operations: &[],
        unsupported_events: &reached,
    })
    .unwrap();
    let source = closure
        .iter()
        .find(|path| path.name == "execution.unsupported-event-source")
        .unwrap();
    assert!(matches!(
        source.status,
        ClosurePathStatus::ExercisedUnsupported
    ));
    assert_eq!(source.unsupported[0].subsystem, "render");
    assert_eq!(
        source.unsupported[0].operation,
        "render.hle-ucode.needs-lle"
    );
    assert_eq!(source.unsupported[0].guest_cycle, Some(42));
    assert_eq!(source.unsupported[0].disposition, "needs_lle");
}

#[test]
fn all_exercised_and_supported_is_release_closed() {
    let mut closure = ClosureGate::default();
    for path in ["cpu", "devices", "render"] {
        closure.declare(path).unwrap();
        closure.observe_supported(path).unwrap();
    }
    ReleaseGateReport::new(
        "synthetic-closed",
        b"synthetic input",
        complete_digest(),
        observations(),
        closure.finish(),
    )
    .unwrap()
    .require_closed()
    .unwrap();
}

#[test]
fn empty_closure_cannot_claim_zero_unsupported() {
    let report = ReleaseGateReport::new(
        "synthetic-empty",
        b"synthetic input",
        complete_digest(),
        observations(),
        Vec::new(),
    )
    .unwrap();
    assert!(matches!(
        report.require_closed(),
        Err(GateError::NoClosurePaths)
    ));
}

#[test]
fn live_closure_is_derived_from_artifacts_and_typed_trace_events() {
    let trace = [
        TraceEvent {
            seq: 1,
            sim_time: 1,
            kind: TraceKind::ThreadSwitch {
                from: None,
                to: 1,
                reason: SwitchReason::Scheduled,
            },
        },
        TraceEvent {
            seq: 2,
            sim_time: 2,
            kind: TraceKind::QueueOp {
                queue: RdramAddr::from_offset(0x100),
                op: QueueOpKind::Send,
                thread: 1,
            },
        },
        TraceEvent {
            seq: 3,
            sim_time: 3,
            kind: TraceKind::Dma {
                direction: DmaDirection::ToRdram,
                dram: RdramAddr::from_offset(0x200),
                device: PiDeviceAddress::RomOffset(0x1000),
                len: 64,
            },
        },
        TraceEvent {
            seq: 4,
            sim_time: 4,
            kind: TraceKind::TaskSubmit {
                task_kind: TaskKind::Graphics,
                ucode: 0x300,
            },
        },
        TraceEvent {
            seq: 5,
            sim_time: 5,
            kind: TraceKind::TaskSubmit {
                task_kind: TaskKind::Audio,
                ucode: 0x400,
            },
        },
    ];
    let device_trace = [
        DeviceTraceEvent {
            at: fn64_runtime::EmulatedInstant::new(5),
            sequence: 1,
            kind: DeviceTraceKind::PiBytesCommitted(PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0x200),
                device: PiDeviceAddress::RomOffset(0x1000),
                len: 64,
            }),
        },
        DeviceTraceEvent {
            at: fn64_runtime::EmulatedInstant::new(6),
            sequence: 2,
            kind: DeviceTraceKind::SiBytesCommitted(SiDmaRequest {
                kind: SiDmaKind::PifToDram,
                dram_addr: RdramAddr::from_offset(0x300),
            }),
        },
        DeviceTraceEvent {
            at: fn64_runtime::EmulatedInstant::new(7),
            sequence: 3,
            kind: DeviceTraceKind::AiDmaComplete(AiDmaRequest {
                dram_addr: RdramAddr::from_offset(0x400),
                len: 2240,
                sample_rate_hz: 32_000,
            }),
        },
        DeviceTraceEvent {
            at: fn64_runtime::EmulatedInstant::new(8),
            sequence: 4,
            kind: DeviceTraceKind::SpTaskAdmitted {
                task_addr: RdramAddr::from_offset(0x500),
                header: OsTaskHeader {
                    task_type: fn64_runtime::M_GFXTASK,
                    ucode_boot: 0x8000_1000,
                    ucode_boot_size: 0x100,
                    ..OsTaskHeader::default()
                },
            },
        },
    ];
    let closure = derive_live_closure(LiveClosureInputs {
        framebuffer_bytes: b"fb",
        audio_bytes: b"pcm",
        memory_bytes: b"memory",
        trace: &trace,
        device_trace: &device_trace,
        save_operations: &[],
        controller_operations: &[],
        unsupported_events: &[],
    })
    .unwrap();
    assert_eq!(closure.len(), LIVE_MINIMUM_CLOSURE_PATHS.len());
    assert!(closure.iter().all(|path| {
        path.observations > 0
            && matches!(path.status, ClosurePathStatus::ExercisedZeroUnsupported)
    }));
}

#[test]
fn live_closure_derives_positive_save_paths_by_authoritative_device_type() {
    let save_operations = [
        SaveOperationEvent {
            at: Cycles::new(2),
            device: SaveType::Eeprom4k,
            operation: SaveOperationKind::Read,
            offset: 0,
            len: 8,
        },
        SaveOperationEvent {
            at: Cycles::new(3),
            device: SaveType::Eeprom4k,
            operation: SaveOperationKind::Write,
            offset: 8,
            len: 8,
        },
        SaveOperationEvent {
            at: Cycles::new(4),
            device: SaveType::Eeprom16k,
            operation: SaveOperationKind::Read,
            offset: 0,
            len: 8,
        },
        SaveOperationEvent {
            at: Cycles::new(5),
            device: SaveType::SramBanked,
            operation: SaveOperationKind::Write,
            offset: 0x20,
            len: 32,
        },
        SaveOperationEvent {
            at: Cycles::new(6),
            device: SaveType::FlashRam,
            operation: SaveOperationKind::Erase,
            offset: 0,
            len: 16 * 1024,
        },
        SaveOperationEvent {
            at: Cycles::new(7),
            device: SaveType::ControllerPak,
            operation: SaveOperationKind::Read,
            offset: 0,
            len: 32,
        },
    ];
    let closure = derive_live_closure(LiveClosureInputs {
        framebuffer_bytes: b"",
        audio_bytes: b"",
        memory_bytes: b"",
        trace: &[],
        device_trace: &[],
        save_operations: &save_operations,
        controller_operations: &[],
        unsupported_events: &[],
    })
    .unwrap();

    let eeprom = closure
        .iter()
        .find(|path| path.name == "save.eeprom-4k-operation")
        .unwrap();
    assert_eq!(eeprom.observations, 2);
    assert!(matches!(
        eeprom.status,
        ClosurePathStatus::ExercisedZeroUnsupported
    ));
    assert!(closure
        .iter()
        .any(|path| path.name == "save.flashram-operation" && path.observations == 1));
    assert!(closure
        .iter()
        .any(|path| path.name == "save.eeprom-16k-operation" && path.observations == 1));
    assert!(closure
        .iter()
        .any(|path| path.name == "save.sram-operation" && path.observations == 1));
    assert!(closure
        .iter()
        .any(|path| path.name == "save.pfs-operation" && path.observations == 1));
}

#[test]
fn live_closure_derives_controller_paths_only_from_successful_operations() {
    let controller_operations = [
        ControllerOperationEvent {
            at: Cycles::new(2),
            port: 0,
            device: ControllerOperationDevice::StandardController,
            operation: fn64_runtime::ControllerOperationKind::Read,
        },
        ControllerOperationEvent {
            at: Cycles::new(3),
            port: 0,
            device: ControllerOperationDevice::RumblePak,
            operation: fn64_runtime::ControllerOperationKind::Control,
        },
        ControllerOperationEvent {
            at: Cycles::new(4),
            port: 1,
            device: ControllerOperationDevice::TransferPak,
            operation: fn64_runtime::ControllerOperationKind::Write,
        },
        ControllerOperationEvent {
            at: Cycles::new(5),
            port: 2,
            device: ControllerOperationDevice::VoiceRecognitionUnit,
            operation: fn64_runtime::ControllerOperationKind::Read,
        },
    ];
    let closure = derive_live_closure(LiveClosureInputs {
        framebuffer_bytes: b"",
        audio_bytes: b"",
        memory_bytes: b"",
        trace: &[],
        device_trace: &[],
        save_operations: &[],
        controller_operations: &controller_operations,
        unsupported_events: &[],
    })
    .unwrap();

    for (_, path) in LIVE_CONTROLLER_OPERATION_CLOSURE_PATHS {
        let evidence = closure
            .iter()
            .find(|candidate| candidate.name == path)
            .unwrap();
        assert_eq!(evidence.observations, 1);
        assert_eq!(evidence.status, ClosurePathStatus::ExercisedZeroUnsupported);
    }
}

#[test]
fn schema_v29_rsp_rdp_wire_rejects_tamper_future_cycles_and_false_graphics_closure() {
    let geometry = observations();
    let graphics_closure = vec![ClosurePath {
        name: "rsp.graphics-task".to_owned(),
        observations: 1,
        status: ClosurePathStatus::ExercisedZeroUnsupported,
        unsupported: Vec::new(),
    }];
    let ordered = vec![
        RspRdpObservationEventEvidence {
            guest_cycle: 40,
            observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                task_address: 0x1000,
                imem_generation: 3,
                text_sha256: "11".repeat(32),
                data_address: 0x2000,
                data_bytes: 0x80,
                data_sha256: "12".repeat(32),
                family: Some(ReleaseMicrocodeFamily::F3dex2),
            },
        },
        RspRdpObservationEventEvidence {
            guest_cycle: 41,
            observation: RspRdpObservationKindEvidence::DramDpcCommitted {
                start: 0x100,
                end: 0x108,
                command_sha256: "22".repeat(32),
            },
        },
        RspRdpObservationEventEvidence {
            guest_cycle: 42,
            observation: RspRdpObservationKindEvidence::ImemReplacementCommitted {
                task_address: 0x1000,
                imem_generation: 4,
                text_sha256: "33".repeat(32),
            },
        },
    ];
    let report = ReleaseGateReport::new_with_environment(
        "rsp-rdp-wire",
        b"input",
        complete_digest(),
        ReleaseBoundaryReportEvidence {
            rom: None,
            observations: geometry.clone(),
            environment: test_release_environment(&geometry),
            execution_destinations: ExecutionDestinationEvidence::no_program(),
            rsp_rdp: RspRdpEvidence::from_ordered(ordered).unwrap(),
        },
        graphics_closure.clone(),
    )
    .unwrap();
    report.verify_integrity().unwrap();

    let mut changed_data_events = report.rsp_rdp.ordered.clone();
    let RspRdpObservationKindEvidence::MicrocodeRecognition { data_sha256, .. } =
        &mut changed_data_events[0].observation
    else {
        panic!("first fixture event must be microcode recognition");
    };
    *data_sha256 = "13".repeat(32);
    let mut changed_data = report.clone();
    changed_data.rsp_rdp = RspRdpEvidence::from_ordered(changed_data_events).unwrap();
    assert!(matches!(
        changed_data.verify_integrity(),
        Err(GateError::ReportIntegrityMismatch { .. })
    ));

    let mut reordered = report.clone();
    reordered.rsp_rdp.ordered.swap(0, 1);
    assert!(matches!(
        reordered.verify_integrity(),
        Err(GateError::NonMonotonicRspRdpObservationCycle {
            previous: 41,
            observed: 40
        })
    ));

    let mut future = report.clone();
    future.rsp_rdp.ordered[0].guest_cycle = 43;
    assert!(matches!(
        future.verify_integrity(),
        Err(GateError::FutureRspRdpObservation {
            gate_cycle: 42,
            event_cycle: 43
        })
    ));

    let mut nonmonotonic_cycle_events = report.rsp_rdp.ordered.clone();
    nonmonotonic_cycle_events[1].guest_cycle = 39;
    let mut nonmonotonic_cycle = report.clone();
    nonmonotonic_cycle.rsp_rdp =
        RspRdpEvidence::from_ordered(nonmonotonic_cycle_events).unwrap();
    assert!(matches!(
        nonmonotonic_cycle.verify_integrity(),
        Err(GateError::NonMonotonicRspRdpObservationCycle {
            previous: 40,
            observed: 39
        })
    ));

    let mut regressing_generation_events = report.rsp_rdp.ordered.clone();
    if let RspRdpObservationKindEvidence::ImemReplacementCommitted {
        imem_generation, ..
    } = &mut regressing_generation_events[2].observation
    {
        *imem_generation = 2;
    }
    let mut regressing_generation = report.clone();
    regressing_generation.rsp_rdp =
        RspRdpEvidence::from_ordered(regressing_generation_events).unwrap();
    assert!(matches!(
        regressing_generation.verify_integrity(),
        Err(GateError::NonMonotonicImemReplacementGeneration {
            previous: 3,
            observed: 2
        })
    ));

    let mut conflicting_digest_events = report.rsp_rdp.ordered.clone();
    conflicting_digest_events.push(RspRdpObservationEventEvidence {
        guest_cycle: 42,
        observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
            task_address: 0x1000,
            imem_generation: 4,
            text_sha256: "44".repeat(32),
            data_address: 0x2000,
            data_bytes: 0x80,
            data_sha256: "12".repeat(32),
            family: None,
        },
    });
    let mut conflicting_digest = report.clone();
    conflicting_digest.rsp_rdp =
        RspRdpEvidence::from_ordered(conflicting_digest_events).unwrap();
    assert!(matches!(
        conflicting_digest.verify_integrity(),
        Err(GateError::ConflictingImemGenerationDigest { generation: 4, .. })
    ));

    let mut invalid_range = report.clone();
    invalid_range.rsp_rdp.ordered[1].observation =
        RspRdpObservationKindEvidence::DramDpcCommitted {
            start: 0x101,
            end: 0x108,
            command_sha256: "22".repeat(32),
        };
    assert!(matches!(
        invalid_range.verify_integrity(),
        Err(GateError::InvalidDpcObservationRange { source: "DRAM", .. })
    ));

    let mut host_only_dram_range = report.clone();
    host_only_dram_range.rsp_rdp.ordered[1].observation =
        RspRdpObservationKindEvidence::DramDpcCommitted {
            start: crate::DEFAULT_RDRAM_SIZE as u32,
            end: crate::DEFAULT_RDRAM_SIZE as u32 + 8,
            command_sha256: "22".repeat(32),
        };
    assert!(matches!(
        host_only_dram_range.verify_integrity(),
        Err(GateError::InvalidDpcObservationRange { source: "DRAM", .. })
    ));

    for (data_address, data_bytes) in [
        (0x2000, 0),
        (crate::DEFAULT_RDRAM_SIZE as u32, 1),
        (crate::DEFAULT_RDRAM_SIZE as u32 - 0x40, 0x80),
        (u32::MAX - 3, 8),
    ] {
        let mut invalid_data_range = report.clone();
        let RspRdpObservationKindEvidence::MicrocodeRecognition {
            data_address: address,
            data_bytes: bytes,
            ..
        } = &mut invalid_data_range.rsp_rdp.ordered[0].observation
        else {
            panic!("first fixture event must be microcode recognition");
        };
        *address = data_address;
        *bytes = data_bytes;
        assert!(matches!(
            invalid_data_range.verify_integrity(),
            Err(GateError::InvalidMicrocodeDataObservationRange { .. })
        ));
    }

    let mut invalid_recognition_task = report.clone();
    let RspRdpObservationKindEvidence::MicrocodeRecognition { task_address, .. } =
        &mut invalid_recognition_task.rsp_rdp.ordered[0].observation
    else {
        panic!("first fixture event must be microcode recognition");
    };
    *task_address = crate::DEFAULT_RDRAM_SIZE as u32 - 63;
    assert!(matches!(
        invalid_recognition_task.verify_integrity(),
        Err(GateError::InvalidRspTaskObservationAddress { .. })
    ));

    let mut invalid_replacement_task = report.clone();
    let RspRdpObservationKindEvidence::ImemReplacementCommitted { task_address, .. } =
        &mut invalid_replacement_task.rsp_rdp.ordered[2].observation
    else {
        panic!("third fixture event must be IMEM replacement");
    };
    *task_address = u32::MAX;
    assert!(matches!(
        invalid_replacement_task.verify_integrity(),
        Err(GateError::InvalidRspTaskObservationAddress { .. })
    ));

    assert!(matches!(
        ReleaseGateReport::new_with_environment(
            "false-graphics-closure",
            b"input",
            complete_digest(),
            ReleaseBoundaryReportEvidence {
                rom: None,
                observations: geometry.clone(),
                environment: test_release_environment(&geometry),
                execution_destinations: ExecutionDestinationEvidence::no_program(),
                rsp_rdp: RspRdpEvidence::from_ordered(Vec::new()).unwrap(),
            },
            graphics_closure,
        ),
        Err(GateError::MissingGraphicsMicrocodeRecognition)
    ));
}

#[test]
fn controller_operation_cycle_validation_rejects_future_evidence() {
    let operation = ControllerOperationEvent {
        at: Cycles::new(43),
        port: 2,
        device: ControllerOperationDevice::TransferPak,
        operation: fn64_runtime::ControllerOperationKind::Read,
    };
    assert!(matches!(
        validate_controller_operation_cycles(42, &[operation]),
        Err(GateError::FutureControllerOperationEvent {
            gate_cycle: 42,
            event_cycle: 43,
            port: 2,
        })
    ));
    assert!(validate_controller_operation_cycles(43, &[operation]).is_ok());
}

#[test]
fn empty_live_artifact_remains_unexercised() {
    let closure = derive_live_closure(LiveClosureInputs {
        framebuffer_bytes: b"fb",
        audio_bytes: b"",
        memory_bytes: b"memory",
        trace: &[],
        device_trace: &[],
        save_operations: &[],
        controller_operations: &[],
        unsupported_events: &[],
    })
    .unwrap();
    let audio = closure.iter().find(|path| path.name == "ai.pcm").unwrap();
    assert_eq!(audio.observations, 0);
    assert!(matches!(audio.status, ClosurePathStatus::Unexercised));
}

#[test]
fn reference_framebuffer_must_match_boundary_owned_memory() {
    let observations = ReleaseObservationGeometry::reference_rdram(4, 1, 1).unwrap();
    let mut memory = vec![0; crate::DEFAULT_RDRAM_SIZE];
    memory[4..6].copy_from_slice(&[0x12, 0x34]);
    assert!(validate_reference_framebuffer_against_memory(
        &observations,
        &[0x12, 0x34],
        &memory,
    )
    .is_ok());
    assert!(matches!(
        validate_reference_framebuffer_against_memory(&observations, &[0xde, 0xad], &memory,),
        Err(GateError::ReferenceFramebufferDoesNotMatchFrozenMemory {
            address: 4,
            bytes: 2,
        })
    ));
}

#[test]
fn generic_executor_dma_cannot_satisfy_device_qualified_closure() {
    let trace = [TraceEvent {
        seq: 1,
        sim_time: 1,
        kind: TraceKind::Dma {
            direction: DmaDirection::ToRdram,
            dram: RdramAddr::from_offset(0x200),
            device: PiDeviceAddress::RomOffset(0x1000),
            len: 64,
        },
    }];
    let closure = derive_live_closure(LiveClosureInputs {
        framebuffer_bytes: b"fb",
        audio_bytes: b"pcm",
        memory_bytes: b"memory",
        trace: &trace,
        device_trace: &[],
        save_operations: &[],
        controller_operations: &[],
        unsupported_events: &[],
    })
    .unwrap();
    assert!(closure
        .iter()
        .filter(|path| path.name.starts_with("device."))
        .all(|path| matches!(path.status, ClosurePathStatus::Unexercised)));
}

#[test]
fn accepted_device_dma_does_not_claim_committed_bytes() {
    let device_trace = [DeviceTraceEvent {
        at: fn64_runtime::EmulatedInstant::new(1),
        sequence: 1,
        kind: DeviceTraceKind::PiDmaStarted(PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x200),
            device: PiDeviceAddress::RomOffset(0x1000),
            len: 64,
        }),
    }];
    let closure = derive_live_closure(LiveClosureInputs {
        framebuffer_bytes: b"fb",
        audio_bytes: b"pcm",
        memory_bytes: b"memory",
        trace: &[],
        device_trace: &device_trace,
        save_operations: &[],
        controller_operations: &[],
        unsupported_events: &[],
    })
    .unwrap();
    let pi = closure
        .iter()
        .find(|path| path.name == "device.pi-dma-commit")
        .unwrap();
    assert_eq!(pi.observations, 0);
    assert!(matches!(pi.status, ClosurePathStatus::Unexercised));
}
