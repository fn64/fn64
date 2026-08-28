use super::*;

#[test]
fn device_state_v18_binds_ai_fifo_identity_and_os_time_bias_in_golden_wire() {
    let bytes = encode_device_snapshot(
        snapshot(42),
        executor_snapshot(),
        host_snapshot(),
        crate::ProgramEvidenceSnapshot::NoProgram,
    );
    assert_eq!(bytes.len(), 8_876);
    // V18 retains V17's AI identities and adds the OSTime bias to the
    // executor control projection.
    assert_eq!(
        sha256_hex(&bytes),
        "03bc09bf6a85b106bbceb3ae84a832af6a59364e6490be7ae6c90cf1254281b9"
    );
}

#[test]
fn device_state_v16_distinguishes_equal_rom_and_sram_offsets() {
    let encoded = |device| {
        let mut state = snapshot(42);
        state.pending_pi = Some(fn64_runtime::PendingPiSnapshot {
            token: 7,
            request: PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0x20),
                device,
                len: 4,
            },
        });
        try_encode_device_component_v16(state).unwrap()
    };
    let rom = encoded(PiDeviceAddress::RomOffset(0x10));
    let sram = encoded(PiDeviceAddress::SramOffset(0x10));
    assert_ne!(rom, sram);
    assert_ne!(sha256_hex(&rom), sha256_hex(&sram));
}

#[test]
fn operational_component_digests_isolate_device_executor_and_abi_host() {
    let device = snapshot(42);
    let executor = executor_snapshot();
    let host = host_snapshot();
    let baseline =
        operational_state_component_digests_v1(device.clone(), executor.clone(), host.clone())
            .unwrap();

    let mut changed_device = device.clone();
    changed_device.guest.pi_status ^= 1;
    let changed =
        operational_state_component_digests_v1(changed_device, executor.clone(), host.clone())
            .unwrap();
    assert_ne!(changed.device_sha256, baseline.device_sha256);
    assert_eq!(changed.executor_sha256, baseline.executor_sha256);
    assert_eq!(changed.abi_host_sha256, baseline.abi_host_sha256);

    let mut changed_executor = executor.clone();
    changed_executor.sim_time += 1;
    let changed =
        operational_state_component_digests_v1(device.clone(), changed_executor, host.clone())
            .unwrap();
    assert_eq!(changed.device_sha256, baseline.device_sha256);
    assert_ne!(changed.executor_sha256, baseline.executor_sha256);
    assert_eq!(changed.abi_host_sha256, baseline.abi_host_sha256);

    let mut changed_host = host.clone();
    changed_host.debug_hardware = fn64_abi::DebugHardware::Msp;
    let changed =
        operational_state_component_digests_v1(device, executor, changed_host).unwrap();
    assert_eq!(changed.device_sha256, baseline.device_sha256);
    assert_eq!(changed.executor_sha256, baseline.executor_sha256);
    assert_ne!(changed.abi_host_sha256, baseline.abi_host_sha256);
}

#[cfg(feature = "recomp-rs")]
#[test]
fn operational_thread_publication_digests_have_stable_golden_wire() {
    use fn64_abi::recompiled::{
        CanonicalThreadCheckpointEvidenceV1, CanonicalThreadPublicationV1,
    };

    let publications = vec![
        CanonicalThreadPublicationV1::Exact(CanonicalThreadCheckpointEvidenceV1 {
            thread: 1,
            cpu: publication_cpu_snapshot(0x1020_3040_5060_7080),
            charged_instructions: 7,
            canonical_charged_instructions_at_publication: 0x0102_0304_0506_0708,
            pending_exit: fn64_cpu_runtime::BlockExit::ResolveCall {
                source_bank: fn64_cpu_runtime::BankId::new(0x1122_3344_5566_7788),
                target_pc: fn64_cpu_runtime::GuestPc::new(0x8000_1000),
                resume: publication_key(9, 0x8000_1008),
            },
            prepared_continuation: None,
        }),
        CanonicalThreadPublicationV1::Exact(CanonicalThreadCheckpointEvidenceV1 {
            thread: 2,
            cpu: publication_cpu_snapshot(0x2030_4050_6070_8090),
            charged_instructions: 3,
            canonical_charged_instructions_at_publication: 0x1112_1314_1516_1718,
            pending_exit: fn64_cpu_runtime::BlockExit::ImageChanged {
                at: publication_key(11, 0x8000_1800),
                miss: fn64_cpu_runtime::AotMiss {
                    expected_bank: fn64_cpu_runtime::BankId::new(11),
                    va_start: fn64_cpu_runtime::GuestPc::new(0x8000_1800),
                    byte_len: 4,
                    expected_sha256: [0x21; 32],
                    actual_sha256: [0x22; 32],
                    first_diff_offset: None,
                },
            },
            prepared_continuation: Some(
                fn64_abi::recompiled::CanonicalPreparedContinuationV1::ImageChanged {
                    entry: publication_key(12, 0x8000_1800),
                },
            ),
        }),
        CanonicalThreadPublicationV1::Exact(CanonicalThreadCheckpointEvidenceV1 {
            thread: 3,
            cpu: publication_cpu_snapshot(0x3040_5060_7080_90a0),
            charged_instructions: 5,
            canonical_charged_instructions_at_publication: 0x2122_2324_2526_2728,
            pending_exit: fn64_cpu_runtime::BlockExit::Fault(fn64_cpu_runtime::CpuFault {
                at: publication_key(13, 0x8000_1a00),
                kind: fn64_cpu_runtime::CpuFaultKind::NoActiveGeneration,
            }),
            prepared_continuation: Some(
                fn64_abi::recompiled::CanonicalPreparedContinuationV1::InactiveGeneration {
                    entry: publication_key(14, 0x8000_1a00),
                },
            ),
        }),
        CanonicalThreadPublicationV1::OpaqueHostInFlight {
            thread: 4,
            target: fn64_cpu_runtime::GuestPc::new(0x8000_2000),
            resume: publication_key(10, 0x8000_2008),
        },
        CanonicalThreadPublicationV1::ParkedFaultOpaque {
            thread: 7,
            post_exception_cpu: publication_cpu_snapshot(0x7080_90a0_b0c0_d0e0),
            fault: fn64_cpu_runtime::CpuFault {
                at: publication_key(15, 0x8000_2800),
                kind: fn64_cpu_runtime::CpuFaultKind::Exception {
                    exception: fn64_cpu_runtime::CpuException::Breakpoint,
                    epc: fn64_cpu_runtime::GuestPc::new(0x8000_2800),
                    branch_delay: false,
                    instruction_code: 0x1020_3040,
                    bad_vaddr: None,
                    coprocessor: None,
                },
            },
            canonical_charged_instructions_at_publication: 0x3132_3334_3536_3738,
        },
        CanonicalThreadPublicationV1::Returned {
            thread: 9,
            cpu: publication_cpu_snapshot(0x8877_6655_4433_2211),
        },
    ];

    let digests = operational_thread_publication_digests_v1(&publications).unwrap();
    assert_eq!(digests.publication_count, 6);
    assert_eq!(digests.exact_count, 3);
    assert_eq!(digests.opaque_count, 2);
    assert_eq!(digests.opaque_host_count, 1);
    assert_eq!(digests.parked_fault_count, 1);
    assert_eq!(digests.returned_count, 1);
    assert_eq!(
        publication_digest_hex(digests.cpu_sha256),
        "c73038499cc70ece7de7c98ff89f6f10fbc966045eb120a6a3f557d42735f5ae"
    );
    assert_eq!(
        publication_digest_hex(digests.continuation_sha256),
        "859d218a586ae04426c0f4ec18cab9a8f3ffedbc9a587670a44f224c8c415f08"
    );
}

#[cfg(feature = "recomp-rs")]
#[test]
fn operational_thread_publication_digests_isolate_cpu_and_continuation() {
    use fn64_abi::recompiled::{
        CanonicalThreadCheckpointEvidenceV1, CanonicalThreadPublicationV1,
    };

    let baseline_publications = vec![CanonicalThreadPublicationV1::Exact(
        CanonicalThreadCheckpointEvidenceV1 {
            thread: 3,
            cpu: publication_cpu_snapshot(11),
            charged_instructions: 2,
            canonical_charged_instructions_at_publication: 101,
            pending_exit: fn64_cpu_runtime::BlockExit::Checkpoint(publication_key(
                5,
                0x8000_3000,
            )),
            prepared_continuation: None,
        },
    )];
    let baseline = operational_thread_publication_digests_v1(&baseline_publications).unwrap();

    let mut changed_cpu = baseline_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut changed_cpu[0] else {
        unreachable!();
    };
    checkpoint.cpu.tlb_entries[31].entry_lo1 ^= 1;
    let changed_cpu = operational_thread_publication_digests_v1(&changed_cpu).unwrap();
    assert_ne!(changed_cpu.cpu_sha256, baseline.cpu_sha256);
    assert_eq!(
        changed_cpu.continuation_sha256,
        baseline.continuation_sha256
    );

    let mut changed_continuation = baseline_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut changed_continuation[0] else {
        unreachable!();
    };
    checkpoint.charged_instructions += 1;
    checkpoint.canonical_charged_instructions_at_publication += 1;
    checkpoint.pending_exit = fn64_cpu_runtime::BlockExit::Yield(publication_key(5, 0x8000_3004));
    let changed_continuation =
        operational_thread_publication_digests_v1(&changed_continuation).unwrap();
    assert_eq!(changed_continuation.cpu_sha256, baseline.cpu_sha256);
    assert_ne!(
        changed_continuation.continuation_sha256,
        baseline.continuation_sha256
    );

    let mut changed_cumulative = baseline_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut changed_cumulative[0] else {
        unreachable!();
    };
    checkpoint.canonical_charged_instructions_at_publication += 1;
    let changed_cumulative =
        operational_thread_publication_digests_v1(&changed_cumulative).unwrap();
    assert_eq!(changed_cumulative.cpu_sha256, baseline.cpu_sha256);
    assert_ne!(
        changed_cumulative.continuation_sha256,
        baseline.continuation_sha256
    );

    let mut image_base_publications = baseline_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut image_base_publications[0]
    else {
        unreachable!();
    };
    checkpoint.pending_exit = fn64_cpu_runtime::BlockExit::ImageChanged {
        at: publication_key(7, 0x8000_3800),
        miss: fn64_cpu_runtime::AotMiss {
            expected_bank: fn64_cpu_runtime::BankId::new(7),
            va_start: fn64_cpu_runtime::GuestPc::new(0x8000_3800),
            byte_len: 4,
            expected_sha256: [0x31; 32],
            actual_sha256: [0x32; 32],
            first_diff_offset: None,
        },
    };
    checkpoint.prepared_continuation = Some(
        fn64_abi::recompiled::CanonicalPreparedContinuationV1::ImageChanged {
            entry: publication_key(7, 0x8000_3800),
        },
    );
    let image_base =
        operational_thread_publication_digests_v1(&image_base_publications).unwrap();
    let mut changed_prepared = image_base_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut changed_prepared[0] else {
        unreachable!();
    };
    checkpoint.prepared_continuation = Some(
        fn64_abi::recompiled::CanonicalPreparedContinuationV1::ImageChanged {
            entry: publication_key(8, 0x8000_3800),
        },
    );
    let changed_prepared =
        operational_thread_publication_digests_v1(&changed_prepared).unwrap();
    assert_eq!(changed_prepared.cpu_sha256, image_base.cpu_sha256);
    assert_ne!(
        changed_prepared.continuation_sha256,
        image_base.continuation_sha256
    );

    let mut inactive_base_publications = baseline_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut inactive_base_publications[0]
    else {
        unreachable!();
    };
    checkpoint.pending_exit = fn64_cpu_runtime::BlockExit::Fault(fn64_cpu_runtime::CpuFault {
        at: publication_key(7, 0x8000_3800),
        kind: fn64_cpu_runtime::CpuFaultKind::NoActiveGeneration,
    });
    checkpoint.prepared_continuation = Some(
        fn64_abi::recompiled::CanonicalPreparedContinuationV1::InactiveGeneration {
            entry: publication_key(7, 0x8000_3800),
        },
    );
    let inactive_base =
        operational_thread_publication_digests_v1(&inactive_base_publications).unwrap();
    let mut changed_inactive_prepared = inactive_base_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut changed_inactive_prepared[0]
    else {
        unreachable!();
    };
    checkpoint.prepared_continuation = Some(
        fn64_abi::recompiled::CanonicalPreparedContinuationV1::InactiveGeneration {
            entry: publication_key(8, 0x8000_3800),
        },
    );
    let changed_inactive_prepared =
        operational_thread_publication_digests_v1(&changed_inactive_prepared).unwrap();
    assert_eq!(
        changed_inactive_prepared.cpu_sha256,
        inactive_base.cpu_sha256
    );
    assert_ne!(
        changed_inactive_prepared.continuation_sha256,
        inactive_base.continuation_sha256
    );

    let opaque_a = [CanonicalThreadPublicationV1::OpaqueHostInFlight {
        thread: 3,
        target: fn64_cpu_runtime::GuestPc::new(0x8000_4000),
        resume: publication_key(6, 0x8000_4008),
    }];
    let opaque_b = [CanonicalThreadPublicationV1::OpaqueHostInFlight {
        thread: 3,
        target: fn64_cpu_runtime::GuestPc::new(0x8000_5000),
        resume: publication_key(6, 0x8000_5008),
    }];
    let opaque_a = operational_thread_publication_digests_v1(&opaque_a).unwrap();
    let opaque_b = operational_thread_publication_digests_v1(&opaque_b).unwrap();
    assert_eq!(opaque_a.cpu_sha256, opaque_b.cpu_sha256);
    assert_ne!(opaque_a.continuation_sha256, opaque_b.continuation_sha256);

    let parked = [CanonicalThreadPublicationV1::ParkedFaultOpaque {
        thread: 3,
        post_exception_cpu: publication_cpu_snapshot(41),
        fault: fn64_cpu_runtime::CpuFault {
            at: publication_key(8, 0x8000_6000),
            kind: fn64_cpu_runtime::CpuFaultKind::Exception {
                exception: fn64_cpu_runtime::CpuException::Breakpoint,
                epc: fn64_cpu_runtime::GuestPc::new(0x8000_6000),
                branch_delay: false,
                instruction_code: 0x1234,
                bad_vaddr: None,
                coprocessor: None,
            },
        },
        canonical_charged_instructions_at_publication: 44,
    }];
    let parked_digest = operational_thread_publication_digests_v1(&parked).unwrap();
    assert_eq!(parked_digest.exact_count, 0);
    assert_eq!(parked_digest.opaque_count, 1);
    assert_eq!(parked_digest.opaque_host_count, 0);
    assert_eq!(parked_digest.parked_fault_count, 1);

    let mut changed_parked_cpu = parked.clone();
    let CanonicalThreadPublicationV1::ParkedFaultOpaque {
        post_exception_cpu, ..
    } = &mut changed_parked_cpu[0]
    else {
        unreachable!();
    };
    *post_exception_cpu = publication_cpu_snapshot(42);
    let changed_parked_cpu =
        operational_thread_publication_digests_v1(&changed_parked_cpu).unwrap();
    assert_ne!(changed_parked_cpu.cpu_sha256, parked_digest.cpu_sha256);
    assert_eq!(
        changed_parked_cpu.continuation_sha256,
        parked_digest.continuation_sha256
    );

    let mut changed_parked_fault = parked.clone();
    let CanonicalThreadPublicationV1::ParkedFaultOpaque { fault, .. } =
        &mut changed_parked_fault[0]
    else {
        unreachable!();
    };
    let fn64_cpu_runtime::CpuFaultKind::Exception {
        instruction_code, ..
    } = &mut fault.kind
    else {
        unreachable!();
    };
    *instruction_code = 0x5678;
    let changed_parked_fault =
        operational_thread_publication_digests_v1(&changed_parked_fault).unwrap();
    assert_eq!(changed_parked_fault.cpu_sha256, parked_digest.cpu_sha256);
    assert_ne!(
        changed_parked_fault.continuation_sha256,
        parked_digest.continuation_sha256
    );

    let mut changed_parked_cumulative = parked.clone();
    let CanonicalThreadPublicationV1::ParkedFaultOpaque {
        canonical_charged_instructions_at_publication,
        ..
    } = &mut changed_parked_cumulative[0]
    else {
        unreachable!();
    };
    *canonical_charged_instructions_at_publication += 1;
    let changed_parked_cumulative =
        operational_thread_publication_digests_v1(&changed_parked_cumulative).unwrap();
    assert_eq!(
        changed_parked_cumulative.cpu_sha256,
        parked_digest.cpu_sha256
    );
    assert_ne!(
        changed_parked_cumulative.continuation_sha256,
        parked_digest.continuation_sha256
    );
}

#[cfg(feature = "recomp-rs")]
#[test]
fn operational_thread_publication_digests_v2_ignore_only_valid_slice_partitioning() {
    use fn64_abi::recompiled::{
        CanonicalPreparedContinuationV1, CanonicalThreadCheckpointEvidenceV1,
        CanonicalThreadPublicationV1,
    };

    let baseline_publications = vec![CanonicalThreadPublicationV1::Exact(
        CanonicalThreadCheckpointEvidenceV1 {
            thread: 3,
            cpu: publication_cpu_snapshot_without_pending_timing(11),
            charged_instructions: 2,
            canonical_charged_instructions_at_publication: 101,
            pending_exit: fn64_cpu_runtime::BlockExit::Checkpoint(publication_key(
                5,
                0x8000_3000,
            )),
            prepared_continuation: None,
        },
    )];
    let baseline = operational_thread_publication_digests_v2(&baseline_publications).unwrap();

    let mut changed_slice = baseline_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut changed_slice[0] else {
        unreachable!();
    };
    checkpoint.charged_instructions = 3;
    checkpoint.cpu.cop0_count ^= 0x0102_0304;
    checkpoint.cpu.cop0_compare ^= 0x1020_3040;
    checkpoint.cpu.cop0_cause ^= fn64_cpu_runtime::CpuInterruptLine::RCP.cause_bit()
        | fn64_cpu_runtime::CpuInterruptLine::TIMER.cause_bit();
    assert_eq!(
        operational_thread_publication_digests_v2(&changed_slice).unwrap(),
        baseline
    );
    assert_ne!(
        operational_thread_publication_digests_v1(&changed_slice)
            .unwrap()
            .cpu_sha256,
        operational_thread_publication_digests_v1(&baseline_publications)
            .unwrap()
            .cpu_sha256
    );
    assert_ne!(
        operational_thread_publication_digests_v1(&changed_slice)
            .unwrap()
            .continuation_sha256,
        operational_thread_publication_digests_v1(&baseline_publications)
            .unwrap()
            .continuation_sha256
    );

    let mut changed_cumulative = baseline_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut changed_cumulative[0] else {
        unreachable!();
    };
    checkpoint.canonical_charged_instructions_at_publication += 1;
    let changed_cumulative =
        operational_thread_publication_digests_v2(&changed_cumulative).unwrap();
    assert_eq!(changed_cumulative.cpu_sha256, baseline.cpu_sha256);
    assert_ne!(
        changed_cumulative.continuation_sha256,
        baseline.continuation_sha256
    );

    let mut changed_cpu = baseline_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut changed_cpu[0] else {
        unreachable!();
    };
    checkpoint.cpu.gprs[1] ^= 1;
    let changed_cpu = operational_thread_publication_digests_v2(&changed_cpu).unwrap();
    assert_ne!(changed_cpu.cpu_sha256, baseline.cpu_sha256);
    assert_eq!(
        changed_cpu.continuation_sha256,
        baseline.continuation_sha256
    );

    let owned_cpu_changes: [fn(&mut fn64_cpu_runtime::RecompContextEvidenceSnapshotV1); 3] = [
        |cpu: &mut fn64_cpu_runtime::RecompContextEvidenceSnapshotV1| cpu.cop0_status ^= 1,
        |cpu: &mut fn64_cpu_runtime::RecompContextEvidenceSnapshotV1| cpu.cop0_random_phase ^= 1,
        |cpu: &mut fn64_cpu_runtime::RecompContextEvidenceSnapshotV1| cpu.cop0_cause ^= 1 << 8,
    ];
    for change in owned_cpu_changes {
        let mut changed_owned_cpu = baseline_publications.clone();
        let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut changed_owned_cpu[0] else {
            unreachable!();
        };
        change(&mut checkpoint.cpu);
        let changed_owned_cpu =
            operational_thread_publication_digests_v2(&changed_owned_cpu).unwrap();
        assert_ne!(changed_owned_cpu.cpu_sha256, baseline.cpu_sha256);
        assert_eq!(
            changed_owned_cpu.continuation_sha256,
            baseline.continuation_sha256
        );
    }

    for count_write in [true, false] {
        let mut pending_timing_write = baseline_publications.clone();
        let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut pending_timing_write[0]
        else {
            unreachable!();
        };
        if count_write {
            checkpoint.cpu.cop0_count_write = Some(1);
        } else {
            checkpoint.cpu.cop0_compare_write = Some(1);
        }
        assert!(matches!(
            operational_thread_publication_digests_v2(&pending_timing_write),
            Err(
                OperationalThreadPublicationDigestErrorV1::PendingCop0TimingWrite { thread: 3 }
            )
        ));
    }

    let mut changed_pending_pc = baseline_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut changed_pending_pc[0] else {
        unreachable!();
    };
    checkpoint.pending_exit =
        fn64_cpu_runtime::BlockExit::Checkpoint(publication_key(5, 0x8000_3004));
    let changed_pending_pc =
        operational_thread_publication_digests_v2(&changed_pending_pc).unwrap();
    assert_eq!(changed_pending_pc.cpu_sha256, baseline.cpu_sha256);
    assert_ne!(
        changed_pending_pc.continuation_sha256,
        baseline.continuation_sha256
    );

    let mut image_publications = baseline_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut image_publications[0] else {
        unreachable!();
    };
    checkpoint.pending_exit = fn64_cpu_runtime::BlockExit::ImageChanged {
        at: publication_key(7, 0x8000_3800),
        miss: fn64_cpu_runtime::AotMiss {
            expected_bank: fn64_cpu_runtime::BankId::new(7),
            va_start: fn64_cpu_runtime::GuestPc::new(0x8000_3800),
            byte_len: 4,
            expected_sha256: [0x31; 32],
            actual_sha256: [0x32; 32],
            first_diff_offset: None,
        },
    };
    checkpoint.prepared_continuation = Some(CanonicalPreparedContinuationV1::ImageChanged {
        entry: publication_key(7, 0x8000_3800),
    });
    let image = operational_thread_publication_digests_v2(&image_publications).unwrap();
    let mut changed_prepared = image_publications.clone();
    let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut changed_prepared[0] else {
        unreachable!();
    };
    checkpoint.prepared_continuation = Some(CanonicalPreparedContinuationV1::ImageChanged {
        entry: publication_key(8, 0x8000_3800),
    });
    let changed_prepared =
        operational_thread_publication_digests_v2(&changed_prepared).unwrap();
    assert_eq!(changed_prepared.cpu_sha256, image.cpu_sha256);
    assert_ne!(
        changed_prepared.continuation_sha256,
        image.continuation_sha256
    );

    for charged_instructions in [0, 102] {
        let mut invalid = baseline_publications.clone();
        let CanonicalThreadPublicationV1::Exact(checkpoint) = &mut invalid[0] else {
            unreachable!();
        };
        checkpoint.charged_instructions = charged_instructions;
        assert!(matches!(
            operational_thread_publication_digests_v2(&invalid),
            Err(
                OperationalThreadPublicationDigestErrorV1::InvalidExactCheckpointCharge {
                    thread: 3
                }
            )
        ));
    }

    let returned = [CanonicalThreadPublicationV1::Returned {
        thread: 3,
        cpu: publication_cpu_snapshot_without_pending_timing(17),
    }];
    let returned_baseline = operational_thread_publication_digests_v2(&returned).unwrap();
    let mut returned_mirrors = returned.clone();
    if let CanonicalThreadPublicationV1::Returned { cpu, .. } = &mut returned_mirrors[0] {
        cpu.cop0_count ^= 1;
        cpu.cop0_compare ^= 1;
        cpu.cop0_cause ^= fn64_cpu_runtime::CpuInterruptLine::RCP.cause_bit()
            | fn64_cpu_runtime::CpuInterruptLine::TIMER.cause_bit();
    } else {
        unreachable!();
    }
    assert_eq!(
        operational_thread_publication_digests_v2(&returned_mirrors).unwrap(),
        returned_baseline
    );
    let CanonicalThreadPublicationV1::Returned { cpu, .. } = &mut returned_mirrors[0] else {
        unreachable!();
    };
    cpu.cop0_count_write = Some(1);
    assert!(matches!(
        operational_thread_publication_digests_v2(&returned_mirrors),
        Err(OperationalThreadPublicationDigestErrorV1::PendingCop0TimingWrite { thread: 3 })
    ));

    let parked = [CanonicalThreadPublicationV1::ParkedFaultOpaque {
        thread: 3,
        post_exception_cpu: publication_cpu_snapshot_without_pending_timing(19),
        fault: fn64_cpu_runtime::CpuFault {
            at: publication_key(8, 0x8000_6000),
            kind: fn64_cpu_runtime::CpuFaultKind::Exception {
                exception: fn64_cpu_runtime::CpuException::Breakpoint,
                epc: fn64_cpu_runtime::GuestPc::new(0x8000_6000),
                branch_delay: false,
                instruction_code: 0x1234,
                bad_vaddr: None,
                coprocessor: None,
            },
        },
        canonical_charged_instructions_at_publication: 44,
    }];
    let parked_baseline = operational_thread_publication_digests_v2(&parked).unwrap();
    let mut parked_mirrors = parked.clone();
    if let CanonicalThreadPublicationV1::ParkedFaultOpaque {
        post_exception_cpu, ..
    } = &mut parked_mirrors[0]
    {
        post_exception_cpu.cop0_count ^= 1;
        post_exception_cpu.cop0_compare ^= 1;
        post_exception_cpu.cop0_cause ^= fn64_cpu_runtime::CpuInterruptLine::RCP.cause_bit()
            | fn64_cpu_runtime::CpuInterruptLine::TIMER.cause_bit();
    } else {
        unreachable!();
    }
    assert_eq!(
        operational_thread_publication_digests_v2(&parked_mirrors).unwrap(),
        parked_baseline
    );
    let CanonicalThreadPublicationV1::ParkedFaultOpaque {
        post_exception_cpu, ..
    } = &mut parked_mirrors[0]
    else {
        unreachable!();
    };
    post_exception_cpu.cop0_compare_write = Some(1);
    assert!(matches!(
        operational_thread_publication_digests_v2(&parked_mirrors),
        Err(OperationalThreadPublicationDigestErrorV1::PendingCop0TimingWrite { thread: 3 })
    ));
}

#[cfg(feature = "recomp-rs")]
#[test]
fn operational_thread_publication_digests_reject_incoherent_native_continuations() {
    use fn64_abi::recompiled::{
        CanonicalPreparedContinuationV1, CanonicalThreadCheckpointEvidenceV1,
        CanonicalThreadPublicationV1,
    };

    let incoherent_prepared = [CanonicalThreadPublicationV1::Exact(
        CanonicalThreadCheckpointEvidenceV1 {
            thread: 3,
            cpu: publication_cpu_snapshot(3),
            charged_instructions: 1,
            canonical_charged_instructions_at_publication: 1,
            pending_exit: fn64_cpu_runtime::BlockExit::Checkpoint(publication_key(
                3,
                0x8000_3000,
            )),
            prepared_continuation: Some(CanonicalPreparedContinuationV1::ImageChanged {
                entry: publication_key(4, 0x8000_4000),
            }),
        },
    )];
    assert!(matches!(
        operational_thread_publication_digests_v1(&incoherent_prepared),
        Err(
            OperationalThreadPublicationDigestErrorV1::IncoherentPreparedContinuation {
                thread: 3
            }
        )
    ));

    let missing_image_continuation = [CanonicalThreadPublicationV1::Exact(
        CanonicalThreadCheckpointEvidenceV1 {
            thread: 3,
            cpu: publication_cpu_snapshot(3),
            charged_instructions: 1,
            canonical_charged_instructions_at_publication: 1,
            pending_exit: fn64_cpu_runtime::BlockExit::ImageChanged {
                at: publication_key(3, 0x8000_3000),
                miss: fn64_cpu_runtime::AotMiss {
                    expected_bank: fn64_cpu_runtime::BankId::new(3),
                    va_start: fn64_cpu_runtime::GuestPc::new(0x8000_3000),
                    byte_len: 4,
                    expected_sha256: [0x31; 32],
                    actual_sha256: [0x32; 32],
                    first_diff_offset: None,
                },
            },
            prepared_continuation: None,
        },
    )];
    assert!(matches!(
        operational_thread_publication_digests_v1(&missing_image_continuation),
        Err(
            OperationalThreadPublicationDigestErrorV1::IncoherentPreparedContinuation {
                thread: 3
            }
        )
    ));

    let missing_inactive_continuation = [CanonicalThreadPublicationV1::Exact(
        CanonicalThreadCheckpointEvidenceV1 {
            thread: 3,
            cpu: publication_cpu_snapshot(3),
            charged_instructions: 1,
            canonical_charged_instructions_at_publication: 1,
            pending_exit: fn64_cpu_runtime::BlockExit::Fault(fn64_cpu_runtime::CpuFault {
                at: publication_key(3, 0x8000_3000),
                kind: fn64_cpu_runtime::CpuFaultKind::NoActiveGeneration,
            }),
            prepared_continuation: None,
        },
    )];
    assert!(matches!(
        operational_thread_publication_digests_v1(&missing_inactive_continuation),
        Err(
            OperationalThreadPublicationDigestErrorV1::IncoherentPreparedContinuation {
                thread: 3
            }
        )
    ));

    let mismatched_pc = [CanonicalThreadPublicationV1::Exact(
        CanonicalThreadCheckpointEvidenceV1 {
            thread: 3,
            cpu: publication_cpu_snapshot(3),
            charged_instructions: 1,
            canonical_charged_instructions_at_publication: 1,
            pending_exit: fn64_cpu_runtime::BlockExit::Fault(fn64_cpu_runtime::CpuFault {
                at: publication_key(3, 0x8000_3000),
                kind: fn64_cpu_runtime::CpuFaultKind::NoActiveGeneration,
            }),
            prepared_continuation: Some(CanonicalPreparedContinuationV1::InactiveGeneration {
                entry: publication_key(4, 0x8000_3004),
            }),
        },
    )];
    assert!(matches!(
        operational_thread_publication_digests_v1(&mismatched_pc),
        Err(
            OperationalThreadPublicationDigestErrorV1::IncoherentPreparedContinuation {
                thread: 3
            }
        )
    ));

    let cross_variant = [CanonicalThreadPublicationV1::Exact(
        CanonicalThreadCheckpointEvidenceV1 {
            thread: 3,
            cpu: publication_cpu_snapshot(3),
            charged_instructions: 1,
            canonical_charged_instructions_at_publication: 1,
            pending_exit: fn64_cpu_runtime::BlockExit::Fault(fn64_cpu_runtime::CpuFault {
                at: publication_key(3, 0x8000_3000),
                kind: fn64_cpu_runtime::CpuFaultKind::NoActiveGeneration,
            }),
            prepared_continuation: Some(CanonicalPreparedContinuationV1::ImageChanged {
                entry: publication_key(4, 0x8000_3000),
            }),
        },
    )];
    assert!(matches!(
        operational_thread_publication_digests_v1(&cross_variant),
        Err(
            OperationalThreadPublicationDigestErrorV1::IncoherentPreparedContinuation {
                thread: 3
            }
        )
    ));

    for (charged_instructions, cumulative) in [(0, 0), (2, 1)] {
        let invalid_charge = [CanonicalThreadPublicationV1::Exact(
            CanonicalThreadCheckpointEvidenceV1 {
                thread: 3,
                cpu: publication_cpu_snapshot(3),
                charged_instructions,
                canonical_charged_instructions_at_publication: cumulative,
                pending_exit: fn64_cpu_runtime::BlockExit::Checkpoint(publication_key(
                    3,
                    0x8000_3000,
                )),
                prepared_continuation: None,
            },
        )];
        assert!(matches!(
            operational_thread_publication_digests_v1(&invalid_charge),
            Err(
                OperationalThreadPublicationDigestErrorV1::InvalidExactCheckpointCharge {
                    thread: 3
                }
            )
        ));
    }

    let non_exception_parked = [CanonicalThreadPublicationV1::ParkedFaultOpaque {
        thread: 4,
        post_exception_cpu: publication_cpu_snapshot(4),
        fault: fn64_cpu_runtime::CpuFault {
            at: publication_key(4, 0x8000_4000),
            kind: fn64_cpu_runtime::CpuFaultKind::UnsupportedInstruction { word: 0 },
        },
        canonical_charged_instructions_at_publication: 2,
    }];
    assert!(matches!(
        operational_thread_publication_digests_v1(&non_exception_parked),
        Err(
            OperationalThreadPublicationDigestErrorV1::ParkedFaultIsNotArchitecturalException {
                thread: 4
            }
        )
    ));
}

#[cfg(feature = "recomp-rs")]
#[test]
fn operational_thread_publication_digests_reject_non_strict_order() {
    use fn64_abi::recompiled::CanonicalThreadPublicationV1;

    for publications in [
        vec![
            CanonicalThreadPublicationV1::Returned {
                thread: 2,
                cpu: publication_cpu_snapshot(2),
            },
            CanonicalThreadPublicationV1::Returned {
                thread: 1,
                cpu: publication_cpu_snapshot(1),
            },
        ],
        vec![
            CanonicalThreadPublicationV1::Returned {
                thread: 2,
                cpu: publication_cpu_snapshot(2),
            },
            CanonicalThreadPublicationV1::Returned {
                thread: 2,
                cpu: publication_cpu_snapshot(3),
            },
        ],
    ] {
        assert!(matches!(
            operational_thread_publication_digests_v1(&publications),
            Err(
                OperationalThreadPublicationDigestErrorV1::NonStrictThreadOrder {
                    index: 1,
                    previous: 2,
                    ..
                }
            )
        ));
    }
}

#[test]
fn operational_component_digests_reject_noncanonical_device_state() {
    let mut device = snapshot(42);
    device.guest.dpc_clock = 0x0100_0000;
    assert!(matches!(
        operational_state_component_digests_v1(device, executor_snapshot(), host_snapshot(),),
        Err(GateError::NonCanonicalDpcCounter {
            register: "DPC_CLOCK",
            value: 0x0100_0000,
        })
    ));
}

#[test]
fn schema_v20_rom_identity_normalizes_byte_order_and_decodes_every_tv_class() {
    let ntsc = test_rom(b'E');
    let expected =
        ReleaseRomEvidence::from_bytes(&ntsc, ReleaseRomClass::RetailCartridge, TvType::Ntsc)
            .unwrap();
    assert_eq!(expected.source_byte_order, ReleaseRomByteOrder::Z64);
    assert_eq!(expected.decoded_tv_region, ReleaseTvRegion::Ntsc);
    assert_eq!(
        ReleaseRomEvidence::decode_tv_type(&ntsc).unwrap(),
        Some(TvType::Ntsc)
    );

    for (bytes, order) in [
        (n64_order(&ntsc), ReleaseRomByteOrder::N64),
        (v64_order(&ntsc), ReleaseRomByteOrder::V64),
    ] {
        let observed = ReleaseRomEvidence::from_bytes(
            &bytes,
            ReleaseRomClass::RetailCartridge,
            TvType::Ntsc,
        )
        .unwrap();
        assert_eq!(observed.source_byte_order, order);
        assert_eq!(observed.canonical_sha256, expected.canonical_sha256);
    }

    let pal = ReleaseRomEvidence::from_bytes(
        &test_rom(b'P'),
        ReleaseRomClass::PublicHomebrew,
        TvType::Pal,
    )
    .unwrap();
    assert_eq!(pal.decoded_tv_region, ReleaseTvRegion::Pal);
    let mpal = ReleaseRomEvidence::from_bytes(
        &test_rom(b'B'),
        ReleaseRomClass::Unclassified,
        TvType::Mpal,
    )
    .unwrap();
    assert_eq!(mpal.decoded_tv_region, ReleaseTvRegion::Mpal);

    for destination_code in [0, b'A'] {
        let region_free = test_rom(destination_code);
        assert_eq!(
            ReleaseRomEvidence::decode_tv_type(&region_free).unwrap(),
            None
        );
        for tv_type in [TvType::Ntsc, TvType::Pal, TvType::Mpal] {
            assert_eq!(
                ReleaseRomEvidence::from_bytes(
                    &region_free,
                    ReleaseRomClass::PublicHomebrew,
                    tv_type,
                )
                .unwrap()
                .decoded_tv_region,
                ReleaseTvRegion::RegionFree
            );
        }
    }
}

#[test]
fn schema_v20_rom_decode_rejects_unknown_or_inconsistent_authority() {
    assert!(matches!(
        ReleaseRomEvidence::from_bytes(
            &test_rom(b'E'),
            ReleaseRomClass::RetailCartridge,
            TvType::Pal,
        ),
        Err(GateError::RomTvTypeMismatch { .. })
    ));
    assert!(matches!(
        ReleaseRomEvidence::decode_tv_type(&test_rom(b'?')),
        Err(GateError::UnknownRomDestinationCode(b'?'))
    ));
    let mut unknown_order = test_rom(b'E');
    unknown_order[..4].fill(0);
    assert!(matches!(
        ReleaseRomEvidence::decode_tv_type(&unknown_order),
        Err(GateError::UnknownRomByteOrder { .. })
    ));
    assert!(matches!(
        ReleaseRomEvidence::decode_tv_type(&[0; 63]),
        Err(GateError::RomTooSmall { bytes: 63 })
    ));
    assert!(matches!(
        ReleaseRomEvidence::decode_tv_type(&[0; 65]),
        Err(GateError::RomNotWordAligned { bytes: 65 })
            | Err(GateError::UnknownRomByteOrder { .. })
    ));
}

#[test]
fn execution_destination_evidence_binds_order_and_collision_safe_unique_counts() {
    let program = crate::ProgramEvidenceSnapshot::IdentifiedNativeArchive(
        crate::NativeProgramArtifactIdentity::new([0x21; 32]),
    );
    let first = native_destination_event(3, 1, 0x10, 0x8000_1010);
    let collision = native_destination_event(4, 2, 0x20, 0x8000_1010);
    let repeated = native_destination_event(5, 1, 0x10, 0x8000_1010);
    let evidence = capture_execution_destinations(
        &program,
        crate::FrozenExecutionDestinations {
            native: vec![first, collision, repeated],
            #[cfg(feature = "recomp-rs")]
            function: Vec::new(),
            #[cfg(feature = "recomp-rs")]
            block: Vec::new(),
        },
        5,
    )
    .unwrap();
    assert_eq!(evidence.total_observations, 3);
    assert_eq!(evidence.unique_destinations, 2);
    assert_eq!(evidence.unique[0].observations, 2);
    assert_eq!(evidence.unique[1].observations, 1);
    evidence.verify_integrity().unwrap();

    let mut reordered = evidence.clone();
    reordered.ordered.swap(0, 1);
    assert!(matches!(
        reordered.verify_integrity(),
        Err(GateError::ExecutionDestinationIntegrityMismatch)
    ));
    let reordered_canonical =
        ExecutionDestinationEvidence::from_ordered(evidence.source.clone(), reordered.ordered)
            .unwrap();
    assert_ne!(evidence.ordered_sha256, reordered_canonical.ordered_sha256);
    assert_eq!(evidence.unique_sha256, reordered_canonical.unique_sha256);

    let geometry = observations();
    let first_report = ReleaseGateReport::new_with_environment(
        "destination-order",
        b"input",
        complete_digest(),
        ReleaseBoundaryReportEvidence {
            rom: None,
            observations: geometry.clone(),
            environment: test_release_environment(&geometry),
            execution_destinations: evidence.clone(),
            rsp_rdp: RspRdpEvidence::from_ordered(Vec::new()).unwrap(),
        },
        Vec::new(),
    )
    .unwrap();
    let second_report = ReleaseGateReport::new_with_environment(
        "destination-order",
        b"input",
        complete_digest(),
        ReleaseBoundaryReportEvidence {
            rom: None,
            observations: geometry.clone(),
            environment: test_release_environment(&geometry),
            execution_destinations: reordered_canonical,
            rsp_rdp: RspRdpEvidence::from_ordered(Vec::new()).unwrap(),
        },
        Vec::new(),
    )
    .unwrap();
    assert_ne!(first_report.report_sha256, second_report.report_sha256);

    let mut mutated = evidence;
    mutated.unique[0].observations += 1;
    assert!(matches!(
        mutated.verify_integrity(),
        Err(GateError::ExecutionDestinationIntegrityMismatch)
    ));
}

#[test]
fn execution_destination_capture_rejects_future_and_cross_lane_evidence() {
    let native = crate::ProgramEvidenceSnapshot::IdentifiedNativeArchive(
        crate::NativeProgramArtifactIdentity::new([0x22; 32]),
    );
    assert!(matches!(
        capture_execution_destinations(
            &native,
            crate::FrozenExecutionDestinations {
                native: vec![native_destination_event(6, 1, 0, 0x8000_1000)],
                #[cfg(feature = "recomp-rs")]
                function: Vec::new(),
                #[cfg(feature = "recomp-rs")]
                block: Vec::new(),
            },
            5,
        ),
        Err(GateError::FutureExecutionDestinationEvent {
            gate_cycle: 5,
            event_cycle: 6,
        })
    ));
    assert!(matches!(
        capture_execution_destinations(
            &crate::ProgramEvidenceSnapshot::NoProgram,
            crate::FrozenExecutionDestinations {
                native: vec![native_destination_event(0, 1, 0, 0x8000_1000)],
                #[cfg(feature = "recomp-rs")]
                function: Vec::new(),
                #[cfg(feature = "recomp-rs")]
                block: Vec::new(),
            },
            0,
        ),
        Err(GateError::ExecutionDestinationSourceMismatch(_))
    ));
}

#[cfg(feature = "recomp-rs")]
#[test]
fn typed_block_destination_requires_runner_identity_and_rejects_native_mix() {
    use fn64_cpu_runtime::{
        BankId, ExecutionDestinationObservation, ExecutionKey, GuestPc, ProgramArtifactIdentity,
    };
    let destination = ExecutionKey::new(BankId::new(0x32), GuestPc::new(0x8000_1000));
    let program = typed_block_program();
    assert!(matches!(
        capture_execution_destinations(
            &program,
            crate::FrozenExecutionDestinations {
                native: Vec::new(),
                function: Vec::new(),
                block: vec![ExecutionDestinationObservation {
                    destination,
                    runner_artifact_identity: None,
                    instructions: 0,
                }],
            },
            0,
        ),
        Err(GateError::UnidentifiedBlockRunnerArtifact { .. })
    ));
    assert!(matches!(
        capture_execution_destinations(
            &program,
            crate::FrozenExecutionDestinations {
                native: vec![native_destination_event(0, 1, 0, 0x8000_1000)],
                function: Vec::new(),
                block: vec![ExecutionDestinationObservation {
                    destination,
                    runner_artifact_identity: Some(ProgramArtifactIdentity::new([0x33; 32])),
                    instructions: 0,
                }],
            },
            0,
        ),
        Err(GateError::ExecutionDestinationSourceMismatch(_))
    ));

    let evidence = capture_execution_destinations(
        &program,
        crate::FrozenExecutionDestinations {
            native: Vec::new(),
            function: Vec::new(),
            block: vec![ExecutionDestinationObservation {
                destination,
                runner_artifact_identity: Some(ProgramArtifactIdentity::new([0x33; 32])),
                instructions: 0,
            }],
        },
        0,
    )
    .unwrap();
    assert!(matches!(
        evidence.source,
        ExecutionDestinationSource::TypedBlockProgram { .. }
    ));
    evidence.verify_integrity().unwrap();
}

#[cfg(feature = "recomp-rs")]
#[test]
fn typed_function_destination_binds_identity_cycle_symbol_order_and_counts() {
    use fn64_abi::recompiled::{
        FunctionExecutionDestinationObservation, RecompiledProgramEvidenceSnapshot,
    };
    use fn64_cpu_runtime::{
        ProgramArtifactIdentity, ProgramIdentityEvidenceSnapshot, ProgramIdentitySource,
        TranslatedFunctionIdentity,
    };

    let identity = ProgramArtifactIdentity::new([0x44; 32]);
    let program = crate::ProgramEvidenceSnapshot::TypedRust(
        RecompiledProgramEvidenceSnapshot::Function {
            identity: ProgramIdentityEvidenceSnapshot {
                identity,
                source: ProgramIdentitySource::CallerSupplied,
            },
        },
    );
    let event = |cycle, vram, symbol| FunctionExecutionDestinationObservation {
        at: fn64_runtime::Cycles::new(cycle),
        artifact_identity: identity,
        function: TranslatedFunctionIdentity::new(vram, symbol),
    };
    let frozen = |function| crate::FrozenExecutionDestinations {
        native: Vec::new(),
        function,
        block: Vec::new(),
    };

    let evidence = capture_execution_destinations(
        &program,
        frozen(vec![
            event(3, 0x8000_1000, "entry"),
            event(4, 0x8000_1000, "alias"),
            event(5, 0x8000_1000, "entry"),
        ]),
        5,
    )
    .unwrap();
    assert_eq!(evidence.total_observations, 3);
    assert_eq!(evidence.unique_destinations, 2);
    assert!(matches!(
        evidence.source,
        ExecutionDestinationSource::TypedObservedFunctionProgram { .. }
    ));
    assert_eq!(evidence.unique[0].observations, 1);
    assert_eq!(evidence.unique[1].observations, 2);
    evidence.verify_integrity().unwrap();
    let json = serde_json::to_value(&evidence).unwrap();
    assert_eq!(
        json["source"],
        serde_json::json!({
            "kind": "typed_observed_function_program",
            "artifact_sha256": "44".repeat(32),
        })
    );
    assert_eq!(
        json["ordered"][0],
        serde_json::json!({
            "guest_cycle": 3,
            "destination": {
                "lane": "typed_function",
                "vram": 0x8000_1000_u32,
                "symbol": "entry",
            },
        })
    );

    let reordered = ExecutionDestinationEvidence::from_ordered(
        evidence.source.clone(),
        vec![
            evidence.ordered[1].clone(),
            evidence.ordered[0].clone(),
            evidence.ordered[2].clone(),
        ],
    )
    .unwrap();
    assert_ne!(evidence.ordered_sha256, reordered.ordered_sha256);
    assert_eq!(evidence.unique_sha256, reordered.unique_sha256);
    let mut tampered = evidence.clone();
    if let ReleaseExecutionDestination::TypedFunction { symbol, .. } =
        &mut tampered.ordered[0].destination
    {
        *symbol = "tampered".to_owned();
    }
    assert!(matches!(
        tampered.verify_integrity(),
        Err(GateError::ExecutionDestinationIntegrityMismatch)
    ));

    assert!(matches!(
        capture_execution_destinations(
            &program,
            frozen(vec![event(6, 0x8000_1000, "entry")]),
            5,
        ),
        Err(GateError::FutureExecutionDestinationEvent { .. })
    ));
    let future_retained = ExecutionDestinationEvidence::from_ordered(
        evidence.source.clone(),
        vec![ExecutionDestinationEventEvidence {
            guest_cycle: Some(6),
            destination: ReleaseExecutionDestination::TypedFunction {
                vram: 0x8000_1000,
                symbol: "entry".to_owned(),
            },
        }],
    )
    .unwrap();
    assert!(matches!(
        validate_execution_destination_cycles(5, &future_retained),
        Err(GateError::FutureExecutionDestinationEvent { .. })
    ));
    let mut wrong_identity = event(5, 0x8000_1000, "entry");
    wrong_identity.artifact_identity = ProgramArtifactIdentity::new([0x45; 32]);
    assert!(matches!(
        capture_execution_destinations(&program, frozen(vec![wrong_identity]), 5),
        Err(GateError::FunctionDestinationArtifactMismatch { .. })
    ));
    assert!(matches!(
        capture_execution_destinations(&program, frozen(Vec::new()), 5),
        Err(GateError::EmptyExecutionDestinationEvidence(
            "typed_observed_function_program"
        ))
    ));
    assert!(matches!(
        capture_execution_destinations(
            &program,
            crate::FrozenExecutionDestinations {
                native: vec![native_destination_event(5, 1, 0, 0x8000_1000)],
                function: vec![event(5, 0x8000_1000, "entry")],
                block: Vec::new(),
            },
            5,
        ),
        Err(GateError::ExecutionDestinationSourceMismatch(_))
    ));
}

#[test]
fn live_gate_rejects_native_execution_destination_before_arm() {
    fn64_abi::load_rom(Vec::new());
    unsafe {
        fn64_abi::register_section(
            0x0010_0000,
            0x8000_1000,
            4,
            &[(0, 4, late_native_destination)],
        );
    }
    fn64_abi::fn64_c_recompiled_function_enter(late_native_destination);
    assert!(matches!(
        LiveReleaseGate::new(0).arm(),
        Err(GateError::LiveGateArmedLate {
            native_execution_destination_events: 1,
            ..
        })
    ));
}

#[cfg(feature = "recomp-rs")]
#[test]
fn live_gate_rejects_function_execution_destination_before_arm() {
    fn lookup(_vram: u32) -> fn64_cpu_runtime::RecompFunc {
        fn body(
            _ctx: &mut fn64_cpu_runtime::RecompContext,
            _rdram: &mut fn64_cpu_runtime::Rdram<'_>,
        ) {
        }
        body
    }

    std::thread::spawn(|| {
        fn64_abi::load_rom(Vec::new());
        fn64_abi::recompiled::set_entry_lookup_with_execution_observation(
            lookup,
            0x100,
            fn64_cpu_runtime::ProgramArtifactIdentity::new([0x5c; 32]),
            fn64_cpu_runtime::FUNCTION_ENTRY_OBSERVATION_SCHEMA,
        );
        fn64_cpu_runtime::notify_function_entry(fn64_cpu_runtime::TranslatedFunctionIdentity::new(
            0x8000_1000,
            "entry",
        ));
        assert!(matches!(
            LiveReleaseGate::new(0).arm(),
            Err(GateError::LiveGateArmedLate {
                function_execution_destination_events: 1,
                ..
            })
        ));
    })
    .join()
    .unwrap();
}

#[test]
fn schema_v30_fixed_cycle_digest_is_stable_and_complete() {
    assert_eq!(complete_digest(), complete_digest());
    assert_eq!(complete_digest().artifacts.len(), 5);
    // This root includes the internal device-evidence wire pinned above.
    assert_eq!(
        complete_digest().root_sha256,
        "d74d23c6c4c2591c9bddd6f60aec3478caf3d9a8534f5cbabadac3acf53e0b57"
    );
}

#[test]
fn schema_v30_report_wire_binds_rom_identity_class_and_tv_authorities() {
    let input = test_rom(b'E');
    let geometry = observations();
    let rom =
        ReleaseRomEvidence::from_bytes(&input, ReleaseRomClass::RetailCartridge, TvType::Ntsc)
            .unwrap();
    let report = ReleaseGateReport::new_with_environment(
        "rom-wire",
        &input,
        complete_digest(),
        ReleaseBoundaryReportEvidence {
            rom: Some(rom),
            observations: geometry.clone(),
            environment: test_release_environment(&geometry),
            execution_destinations: ExecutionDestinationEvidence::no_program(),
            rsp_rdp: RspRdpEvidence::from_ordered(Vec::new()).unwrap(),
        },
        Vec::new(),
    )
    .unwrap();
    report.verify_integrity().unwrap();

    let baseline = report.report_sha256.clone();
    let mut changed_class = report.clone();
    changed_class.rom.as_mut().unwrap().class = ReleaseRomClass::PublicHomebrew;
    assert_ne!(
        sha256_hex(&encode_report_evidence(&changed_class).unwrap()),
        baseline
    );

    let mut changed_order = report.clone();
    changed_order.rom.as_mut().unwrap().source_byte_order = ReleaseRomByteOrder::V64;
    assert_ne!(
        sha256_hex(&encode_report_evidence(&changed_order).unwrap()),
        baseline
    );

    let mut changed_identity = report.clone();
    changed_identity.rom.as_mut().unwrap().canonical_sha256 = "ab".repeat(32);
    assert_ne!(
        sha256_hex(&encode_report_evidence(&changed_identity).unwrap()),
        baseline
    );

    let mut changed_destination = report.clone();
    changed_destination.rom.as_mut().unwrap().destination_code = b'P';
    assert!(matches!(
        changed_destination.verify_integrity(),
        Err(GateError::RomRegionDecodeMismatch { .. })
    ));

    let mut changed_region = report.clone();
    changed_region.rom.as_mut().unwrap().decoded_tv_region = ReleaseTvRegion::Pal;
    assert!(matches!(
        changed_region.verify_integrity(),
        Err(GateError::RomRegionDecodeMismatch { .. })
    ));

    let mut changed_renderer_tv = report.clone();
    let ReleaseRendererEvidence::Reference { tv_type, .. } =
        &mut changed_renderer_tv.environment.renderer
    else {
        unreachable!()
    };
    *tv_type = ReleaseTvStandard::Mpal;
    changed_renderer_tv.report_sha256 =
        sha256_hex(&encode_report_evidence(&changed_renderer_tv).unwrap());
    assert!(matches!(
        changed_renderer_tv.verify_integrity(),
        Err(GateError::RomTvTypeMismatch {
            authority: "retained renderer create-time configuration",
            ..
        })
    ));

    let mut mismatched_input = input;
    mismatched_input[0x100] ^= 1;
    assert!(matches!(
        ReleaseGateReport::new_with_environment(
            "rom-wire",
            &mismatched_input,
            report.digest,
            ReleaseBoundaryReportEvidence {
                rom: report.rom,
                observations: report.observations,
                environment: report.environment,
                execution_destinations: report.execution_destinations,
                rsp_rdp: report.rsp_rdp,
            },
            Vec::new(),
        ),
        Err(GateError::RomInputEvidenceMismatch)
    ));
}
