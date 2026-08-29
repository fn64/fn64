use super::*;

    #[test]
    fn bootstrap_import_installs_complete_cold_ipl_globals() {
        let install = bootstrap_test_install(0x2402_0001);
        for (tv_type, expected_tv) in [
            (fn64_runtime::TvType::Pal, 0),
            (fn64_runtime::TvType::Ntsc, 1),
            (fn64_runtime::TvType::Mpal, 2),
        ] {
            let transaction = install
                .begin_bootstrap_import_v1(&[], bootstrap_test_rdram_len(), tv_type)
                .unwrap();
            let view = fn64_runtime::RdramView::from_storage(&transaction.storage);
            assert_eq!(view.read_u32(fn64_runtime::OS_TV_TYPE_ADDR), expected_tv);
            assert_eq!(
                view.read_u32(fn64_runtime::OS_ROM_BASE_ADDR),
                fn64_runtime::CART_ROM_KSEG1_BASE
            );
            assert_eq!(view.read_u32(fn64_runtime::OS_RESET_TYPE_ADDR), 0);
        }
    }


    #[test]
    fn bootstrap_import_commit_binds_rom_catalog_entry_and_static_watched_bytes() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let evidence = validated.receipt().evidence();
        let expected_rom_sha256: [u8; 32] = sha2::Sha256::digest(&rom).into();

        assert_eq!(validated.len(), bootstrap_test_rdram_len());
        assert_eq!(evidence.rom_sha256, expected_rom_sha256);
        assert_eq!(
            evidence.initial_entry,
            ExecutionKey::new(BankId::new(0xb007), GuestPc::new(0x8000_7000))
        );
        assert_eq!(
            evidence.watched_ranges,
            [PendingExecutableWriteEvidenceSnapshot {
                physical_start: 0x7000,
                physical_end: 0x7004,
            }]
        );
        assert_eq!(evidence.publications.len(), 1);
        assert_ne!(evidence.watched_sha256, [0; 32]);
        assert_ne!(evidence.receipt_sha256, [0; 32]);
    }


    #[test]
    fn bootstrap_writer_channel_receipt_is_minted_from_exact_private_journal_state() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();

        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let state = CanonicalExecutableMutationStateV1::from_bootstrap(
            validated.receipt().evidence(),
            &validated.storage,
        );
        let receipt = validate_bootstrap_writer_completion_state(
            canonical_writer_program_model_sha256(
                &install.resolver,
                Some(&install.generations),
                &validated.receipt().evidence().watched_ranges,
            ),
            validated.receipt().evidence(),
            &validated.storage,
            &state,
        )
        .unwrap();
        let evidence = receipt.evidence();
        assert_eq!(
            evidence.schema,
            BOOTSTRAP_WRITER_CHANNEL_COMPLETION_SCHEMA_V1
        );
        assert!(receipt.has_valid_evidence_hash());
        assert_ne!(receipt.program_model_sha256(), [0; 32]);
        assert_eq!(evidence.journal_entry.sequence, 0);
        assert!(evidence
            .journal_entry
            .declared_writes
            .iter()
            .all(|write| write.channel == WriterChannel::BootstrapOrImport));
        assert_eq!(
            evidence.journal_entry.after_sha256,
            evidence.final_watched_sha256
        );
        assert_eq!(
            evidence.bootstrap_receipt_sha256,
            validated.receipt().evidence().receipt_sha256
        );
        let foreign = bootstrap_test_install(0x2402_0002);
        assert_ne!(
            receipt.program_model_sha256(),
            canonical_writer_program_model_sha256(
                &foreign.resolver,
                Some(&foreign.generations),
                &validated.receipt().evidence().watched_ranges,
            ),
            "writer model identity must bind the canonical BlockProgram image"
        );
    }


    #[test]
    fn bootstrap_writer_channel_validator_rejects_nonquiescent_state() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let bootstrap = validated.receipt().evidence();
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut state =
            CanonicalExecutableMutationStateV1::from_bootstrap(bootstrap, &validated.storage);
        let program_model_sha256 = canonical_writer_program_model_sha256(
            &install.resolver,
            Some(&install.generations),
            &bootstrap.watched_ranges,
        );
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().push((0x7000, 1)));
        assert_eq!(
            validate_bootstrap_writer_completion_state(
                program_model_sha256,
                bootstrap,
                &validated.storage,
                &state,
            )
            .unwrap_err(),
            BootstrapWriterChannelCompletionErrorV1::PendingPhysicalWrites
        );
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| {
            pending.borrow_mut().push(GuestWriteEvent::Range {
                channel: WriterChannel::BootstrapOrImport,
                physical_offset: 0x7000,
                len: 1,
            });
        });
        assert_eq!(
            validate_bootstrap_writer_completion_state(
                program_model_sha256,
                bootstrap,
                &validated.storage,
                &state,
            )
            .unwrap_err(),
            BootstrapWriterChannelCompletionErrorV1::PendingAttributedWrites
        );
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let host = state.begin_host_transaction(
            7,
            GuestPc::new(INSTALL_PC.get() + 4),
            ExecutionKey::new(BankId::new(0xb007), INSTALL_PC),
        );
        assert_eq!(
            validate_bootstrap_writer_completion_state(
                program_model_sha256,
                bootstrap,
                &validated.storage,
                &state,
            )
            .unwrap_err(),
            BootstrapWriterChannelCompletionErrorV1::OpenHostTransactions
        );
        state.finish_host_transaction(host);

        let child = state.begin_child_transaction();
        assert_eq!(
            validate_bootstrap_writer_completion_state(
                program_model_sha256,
                bootstrap,
                &validated.storage,
                &state,
            )
            .unwrap_err(),
            BootstrapWriterChannelCompletionErrorV1::ActiveChildTransaction
        );
        state.finish_child_transaction(child);
        state.poison("synthetic incomplete publication".to_string());
        assert_eq!(
            validate_bootstrap_writer_completion_state(
                program_model_sha256,
                bootstrap,
                &validated.storage,
                &state,
            )
            .unwrap_err(),
            BootstrapWriterChannelCompletionErrorV1::Poisoned
        );
    }


    #[test]
    fn rdp_renderer_writer_receipt_binds_publications_to_exact_journal_sequences() {
        let (storage, state, epoch, trace) = rdp_renderer_validator_fixture(vec![vec![0]]);
        let receipt = validate_rdp_renderer_writer_runtime_state_v1(
            epoch.program_model_sha256,
            [0x73; 32],
            Some([0x74; 32]),
            production_aot_receipt_for_si_test(),
            true,
            &epoch,
            &storage,
            &state,
            &trace,
            false,
            false,
            false,
            false,
        )
        .unwrap();
        assert!(receipt.has_valid_evidence_hash());
        assert_eq!(receipt.evidence().renderer_publication_count, 1);
        assert_eq!(receipt.evidence().rdp_renderer_journal_entry_count, 1);
        assert_eq!(receipt.evidence().rdp_renderer_journal_declaration_count, 1);
    }


    #[test]
    fn rdp_renderer_writer_receipt_rejects_unbound_journal_commit() {
        let (storage, state, epoch, trace) = rdp_renderer_validator_fixture(vec![Vec::new()]);
        assert_eq!(
            validate_rdp_renderer_writer_runtime_state_v1(
                epoch.program_model_sha256,
                [0x73; 32],
                Some([0x74; 32]),
                production_aot_receipt_for_si_test(),
                true,
                &epoch,
                &storage,
                &state,
                &trace,
                false,
                false,
                false,
                false,
            )
            .unwrap_err(),
            RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace
        );
    }


    #[test]
    fn rdp_renderer_writer_receipt_rejects_speculative_needs_lle_write() {
        let (storage, state, epoch, mut trace) = rdp_renderer_validator_fixture(vec![vec![0]]);
        trace.rejected_journal_sequences.push(0);
        assert_eq!(
            validate_rdp_renderer_writer_runtime_state_v1(
                epoch.program_model_sha256,
                [0x73; 32],
                Some([0x74; 32]),
                production_aot_receipt_for_si_test(),
                true,
                &epoch,
                &storage,
                &state,
                &trace,
                false,
                false,
                false,
                false,
            )
            .unwrap_err(),
            RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace
        );
    }


    #[test]
    fn rdp_renderer_writer_rejection_precedes_retryable_empty_publication() {
        let (storage, state, epoch, mut trace) = rdp_renderer_validator_fixture(Vec::new());
        trace.rejected_journal_sequences.push(0);
        assert_eq!(
            validate_rdp_renderer_writer_runtime_state_v1(
                epoch.program_model_sha256,
                [0x73; 32],
                Some([0x74; 32]),
                production_aot_receipt_for_si_test(),
                true,
                &epoch,
                &storage,
                &state,
                &trace,
                false,
                false,
                false,
                false,
            )
            .unwrap_err(),
            RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace
        );
    }


    #[test]
    fn rdp_renderer_writer_receipt_rejects_each_pending_renderer_owner() {
        for (rsp, dpc, dp, abi, expected) in [
            (
                true,
                false,
                false,
                false,
                RdpRendererWriterRuntimeStateErrorV1::PendingDeviceRspTask,
            ),
            (
                false,
                true,
                false,
                false,
                RdpRendererWriterRuntimeStateErrorV1::PendingDeviceDpcTransaction,
            ),
            (
                false,
                false,
                true,
                false,
                RdpRendererWriterRuntimeStateErrorV1::PendingDeviceDpCompletion,
            ),
            (
                false,
                false,
                false,
                true,
                RdpRendererWriterRuntimeStateErrorV1::PendingAbiRendererWork,
            ),
        ] {
            let (storage, state, epoch, trace) = rdp_renderer_validator_fixture(vec![vec![0]]);
            assert_eq!(
                validate_rdp_renderer_writer_runtime_state_v1(
                    epoch.program_model_sha256,
                    [0x73; 32],
                    Some([0x74; 32]),
                    production_aot_receipt_for_si_test(),
                    true,
                    &epoch,
                    &storage,
                    &state,
                    &trace,
                    rsp,
                    dpc,
                    dp,
                    abi,
                )
                .unwrap_err(),
                expected
            );
        }
    }


    #[test]
    fn rdp_renderer_writer_epoch_is_process_unique_across_thread_local_owners() {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let ids = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    next_rdp_renderer_writer_trace_epoch_id()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let values = ids
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert_ne!(values[0], values[1]);
    }


    #[test]
    fn rdp_renderer_writer_public_path_consumes_one_fresh_publication_epoch() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_rdp_renderer_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one fresh renderer epoch");
        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        // SAFETY: the test owner retains the allocation in HostState for the
        // complete scope and no competing slice exists while this call runs.
        let storage = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
        track_rdp_renderer_mutation(storage, |storage| storage[0x7000 ^ 3] ^= 1);
        record_rdp_renderer_publication_v1();

        let receipt = take_validated_rdp_renderer_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .expect("one committed renderer publication must mint one prerequisite");
        assert!(receipt.has_valid_evidence_hash());
        assert_eq!(receipt.evidence().renderer_publication_count, 1);
        assert_eq!(receipt.evidence().rdp_renderer_journal_entry_count, 1);
        assert!(
            take_validated_rdp_renderer_writer_runtime_state_receipt_v1(&epoch)
                .unwrap()
                .is_none()
        );
    }


    #[test]
    fn pi_writer_runtime_state_public_path_owns_fresh_epoch_and_completed_read_dma() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_pi_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one fresh PI epoch");
        assert!(crate::copy_device_trace().is_empty());
        assert!(write_raw_mmio(0xFFFF_FFFF_A460_0000, 0x6000));
        assert!(write_raw_mmio(0xFFFF_FFFF_A460_0004, 0x1000_0020));
        assert!(write_raw_mmio(0xFFFF_FFFF_A460_000C, 3));
        assert_eq!(
            take_validated_pi_writer_runtime_state_receipt_v1(&epoch).unwrap_err(),
            PiWriterRuntimeStateErrorV1::PendingDevicePi
        );

        crate::pi::advance_device_time(1);
        let trace = crate::copy_device_trace();
        assert_eq!(trace.len(), 5);
        assert!(matches!(
            trace.first().map(|event| event.kind),
            Some(fn64_runtime::DeviceTraceKind::PiDmaStarted(
                fn64_runtime::PiDmaRequest {
                    direction: fn64_runtime::DmaDirection::ToRdram,
                    ..
                }
            ))
        ));
        assert!(matches!(
            trace.last().map(|event| event.kind),
            Some(fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::PiDmaComplete(fn64_runtime::DmaCompletion {
                    direction: fn64_runtime::DmaDirection::ToRdram,
                    ..
                })
            ))
        ));
        let receipt = take_validated_pi_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .expect("fresh completed PI lifecycle must mint one runtime-state prerequisite");
        assert_eq!(receipt.evidence().pi_started, 1);
        assert_eq!(receipt.evidence().pi_committed, 1);
        assert_eq!(receipt.evidence().pi_to_rdram_committed, 1);
        assert!(receipt.has_valid_evidence_hash());
        assert!(take_validated_pi_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .is_none());
    }


    #[test]
    fn rsp_writer_runtime_state_public_path_binds_task_owned_writeback() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_rsp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one fresh RSP epoch");
        let owner = crate::task_dispatch::RspInterpreterOwner::RawKick {
            admission_generation: crate::task_dispatch::RspTaskAdmissionGeneration::new(
                std::num::NonZeroU64::new(9).unwrap(),
            ),
        };
        crate::task_dispatch::record_test_rsp_writer_commits_v1(
            crate::task_dispatch::RspWriterCommitSourceV1::Interpreter { owner },
            &[(0x6000, 0x6008)],
        );

        let receipt = take_validated_rsp_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .expect("fresh task-owned RSP writeback must mint one inner receipt");
        assert_eq!(receipt.evidence().interpreter_writeback_count, 1);
        assert_eq!(receipt.evidence().translated_audio_hle_publication_count, 0);
        assert_eq!(receipt.evidence().writeback_range_count, 1);
        assert_eq!(receipt.evidence().rsp_journal_declaration_count, 0);
        assert!(receipt.has_valid_evidence_hash());
        assert!(take_validated_rsp_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .is_none());
    }

    unsafe extern "C" fn translated_audio_test_callback(rdram: *mut u8, _task: u32) -> u32 {
        let physical = (INSTALL_PC.get() & 0x1fff_ffff) as usize;
        unsafe { *rdram.add(physical ^ 3) ^= 1 };
        0
    }

    unsafe extern "C" fn rejected_translated_audio_test_callback(
        rdram: *mut u8,
        _task: u32,
    ) -> u32 {
        let physical = (INSTALL_PC.get() & 0x1fff_ffff) as usize;
        unsafe { *rdram.add(physical ^ 3) ^= 1 };
        9
    }


    #[test]
    fn rsp_writer_runtime_state_credits_real_translated_audio_dispatch() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_rsp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one fresh RSP epoch");

        unsafe {
            crate::task_dispatch::test_dispatch_translated_audio_task_v1(
                0x40,
                translated_audio_test_callback,
            )
        };

        let receipt = take_validated_rsp_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .expect("successful translated audio executable publication must mint a receipt");
        assert_eq!(receipt.evidence().translated_audio_hle_publication_count, 1);
        assert_eq!(receipt.evidence().interpreter_writeback_count, 0);
        assert_eq!(receipt.evidence().writeback_range_count, 0);
        assert_eq!(receipt.evidence().rsp_journal_declaration_count, 1);
        assert!(receipt.has_valid_evidence_hash());
    }


    #[test]
    fn rsp_writer_runtime_state_rejects_real_non_break_audio_dispatch() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_rsp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one fresh RSP epoch");

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            crate::task_dispatch::test_dispatch_translated_audio_task_v1(
                0x40,
                rejected_translated_audio_test_callback,
            )
        }));
        assert!(rejected.is_err());
        with_host(|host| {
            host.rsp_task_lineages.clear();
            host.rsp_interpreter_state =
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::Reset;
        });

        assert_eq!(
            take_validated_rsp_writer_runtime_state_receipt_v1(&epoch).unwrap_err(),
            RspWriterRuntimeStateErrorV1::RejectedRspExecutableMutation
        );
    }


    #[test]
    fn rsp_writer_runtime_state_rejects_pending_owner_and_empty_trace() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let owner = crate::task_dispatch::RspInterpreterOwner::RawKick {
            admission_generation: crate::task_dispatch::RspTaskAdmissionGeneration::new(
                std::num::NonZeroU64::new(10).unwrap(),
            ),
        };
        with_host(|host| {
            host.rsp_interpreter_state =
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { owner };
        });
        assert_eq!(
            begin_rsp_writer_runtime_trace_epoch_v1().unwrap_err(),
            RspWriterRuntimeStateErrorV1::PendingAbiRspWork
        );
        with_host(|host| {
            host.rsp_interpreter_state =
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::Reset;
        });
        let epoch = begin_rsp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("quiescent owner must mint an RSP epoch");
        assert_eq!(
            take_validated_rsp_writer_runtime_state_receipt_v1(&epoch).unwrap_err(),
            RspWriterRuntimeStateErrorV1::NoRspWritebacks
        );
    }


    #[test]
    fn rsp_writer_trace_epoch_ids_are_process_unique_across_threads() {
        let ids = (0..8)
            .map(|_| std::thread::spawn(next_rsp_writer_trace_epoch_id))
            .map(|thread| thread.join().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), 8);
        assert!(ids.iter().all(|id| *id != 0));
    }


    #[test]
    fn pi_writer_runtime_state_rejects_nonwriting_incomplete_and_drifted_lifecycles() {
        assert_eq!(
            validate_pi_transition_trace(&pi_test_trace(fn64_runtime::DmaDirection::FromRdram))
                .unwrap_err(),
            PiWriterRuntimeStateErrorV1::NoToRdramCommit
        );
        let complete = pi_test_trace(fn64_runtime::DmaDirection::ToRdram);
        assert_eq!(
            validate_pi_transition_trace(&complete[..complete.len() - 1]).unwrap_err(),
            PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder
        );
        let mut drifted = complete.clone();
        if let fn64_runtime::DeviceTraceKind::PiBytesCommitted(ref mut request) = drifted[1].kind {
            request.dram_addr = fn64_runtime::RdramAddr::from_offset(0x6010);
        }
        assert_eq!(
            validate_pi_transition_trace(&drifted).unwrap_err(),
            PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder
        );
        let mut nonmonotonic = complete;
        nonmonotonic[4].sequence = 1;
        assert_eq!(
            validate_pi_transition_trace(&nonmonotonic).unwrap_err(),
            PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder
        );
    }


    #[test]
    fn pi_writer_v2_distinguishes_equal_rom_and_sram_offsets() {
        let rom = pi_test_trace_for_device(
            fn64_runtime::DmaDirection::ToRdram,
            fn64_runtime::PiDeviceAddress::RomOffset(0x20),
        );
        let sram = pi_test_trace_for_device(
            fn64_runtime::DmaDirection::ToRdram,
            fn64_runtime::PiDeviceAddress::SramOffset(0x20),
        );
        let rom_digest = validate_pi_transition_trace(&rom).unwrap().7;
        let sram_digest = validate_pi_transition_trace(&sram).unwrap().7;
        assert_ne!(rom_digest, sram_digest);
    }


    #[test]
    fn pi_writer_runtime_state_accepts_serialized_requests_while_interrupt_remains_asserted() {
        let mut trace = pi_test_trace(fn64_runtime::DmaDirection::ToRdram);
        let second = fn64_runtime::PiDmaRequest {
            direction: fn64_runtime::DmaDirection::ToRdram,
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6010),
            device: fn64_runtime::PiDeviceAddress::RomOffset(0x24),
            len: 4,
        };
        let completion = fn64_runtime::DmaCompletion {
            direction: second.direction,
            dram_addr: second.dram_addr,
            device: second.device,
            len: second.len,
        };
        for kind in [
            fn64_runtime::DeviceTraceKind::PiDmaStarted(second),
            fn64_runtime::DeviceTraceKind::PiBytesCommitted(second),
            fn64_runtime::DeviceTraceKind::PiBusyCleared,
            fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::PiDmaComplete(completion),
            ),
        ] {
            let sequence = trace.len() as u64;
            trace.push(fn64_runtime::DeviceTraceEvent {
                at: fn64_runtime::EmulatedInstant::new(200 + sequence),
                sequence,
                kind,
            });
        }
        let (started, committed, busy, raised, cleared, notifications, writes, digest) =
            validate_pi_transition_trace(&trace).unwrap();
        assert_eq!(
            (
                started,
                committed,
                busy,
                raised,
                cleared,
                notifications,
                writes
            ),
            (2, 2, 2, 1, 0, 2, 2)
        );
        assert_ne!(digest, [0; 32]);
    }


    #[test]
    fn pi_writer_runtime_state_rejects_pending_interrupt_and_superseded_epoch() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        with_host(|host| {
            host.device_fabric
                .raise_interrupt(fn64_runtime::InterruptSource::Pi)
        });
        assert_eq!(
            begin_pi_writer_runtime_trace_epoch_v1().unwrap_err(),
            PiWriterRuntimeStateErrorV1::PendingPiInterrupt
        );
        with_host(|host| {
            host.device_fabric
                .clear_interrupt(fn64_runtime::InterruptSource::Pi)
        });
        let old = begin_pi_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("first PI epoch");
        let current = begin_pi_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("replacement PI epoch");
        assert_eq!(
            take_validated_pi_writer_runtime_state_receipt_v1(&old).unwrap_err(),
            PiWriterRuntimeStateErrorV1::TraceEpochMismatch
        );
        assert_eq!(
            take_validated_pi_writer_runtime_state_receipt_v1(&current).unwrap_err(),
            PiWriterRuntimeStateErrorV1::NoPiTransitions
        );
    }


    #[test]
    fn pi_writer_runtime_state_rejects_pending_abi_completion_owner() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_pi_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("fresh PI epoch");
        let (live, storage) = with_host(|host| {
            (
                host.canonical_recompiled_program.clone().unwrap(),
                host.owned_runtime_rdram.as_deref().unwrap().to_vec(),
            )
        });
        assert_eq!(
            live.take_pi_writer_runtime_state(
                &epoch,
                &storage,
                true,
                &pi_test_trace(fn64_runtime::DmaDirection::ToRdram),
                false,
                true,
            )
            .unwrap_err(),
            PiWriterRuntimeStateErrorV1::PendingAbiPi
        );
    }


    #[test]
    fn pi_writer_runtime_state_epoch_ids_are_process_unique_across_threads() {
        let mut ids = (0..16)
            .map(|_| std::thread::spawn(next_pi_writer_trace_epoch_id))
            .map(|thread| thread.join().expect("PI epoch mint thread panicked"))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert!(ids.iter().all(|id| *id != 0));
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }


    #[test]
    fn public_si_runtime_state_path_requires_fresh_completed_device_lifecycle() {
        let _reset = PublicSiRuntimeStateTestReset;
        let request = install_public_si_runtime_state_test_owner();
        crate::set_device_trace_enabled(false);
        crate::set_device_trace_enabled(true);
        assert!(crate::copy_device_trace().is_empty());
        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));

        crate::pi::start_live_si_dma(
            request,
            crate::PendingSiCompletionOwner::ProcessRdram { rdram, rdram_len },
        )
        .unwrap();
        assert_eq!(
            take_validated_si_writer_runtime_state_receipt_v1().unwrap_err(),
            SiWriterRuntimeStateErrorV1::PendingDeviceSi
        );

        crate::pi::advance_device_time(1);
        let trace = crate::copy_device_trace();
        assert_eq!(trace.len(), 5);
        assert!(matches!(
            trace.first().map(|event| event.kind),
            Some(fn64_runtime::DeviceTraceKind::SiDmaStarted(actual)) if actual == request
        ));
        assert!(matches!(
            trace.last().map(|event| event.kind),
            Some(fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::SiDmaComplete(actual)
            )) if actual == request
        ));
        let receipt = take_validated_si_writer_runtime_state_receipt_v1()
            .unwrap()
            .expect("fresh completed SI lifecycle must mint one runtime-state prerequisite");
        assert_eq!(receipt.evidence().si_started, 1);
        assert_eq!(receipt.evidence().si_committed, 1);
        assert_eq!(receipt.evidence().si_pif_to_dram_committed, 1);
        assert!(receipt.has_valid_evidence_hash());
        assert!(take_validated_si_writer_runtime_state_receipt_v1()
            .unwrap()
            .is_none());
    }


    #[test]
    fn sp_writer_runtime_state_public_path_owns_fresh_epoch_and_completed_write_dma() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        assert!(write_raw_mmio(0xFFFF_FFFF_A400_0000, 0x1122_3344));
        let epoch = begin_sp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one fresh SP epoch");
        assert!(crate::copy_device_trace().is_empty());
        assert!(write_raw_mmio(0xFFFF_FFFF_A404_0000, 0));
        assert!(write_raw_mmio(0xFFFF_FFFF_A404_0004, 0x6000));
        assert!(write_raw_mmio(0xFFFF_FFFF_A404_000C, 7));
        assert_eq!(
            take_validated_sp_writer_runtime_state_receipt_v1(&epoch).unwrap_err(),
            SpWriterRuntimeStateErrorV1::PendingDeviceSpDma
        );

        crate::pi::advance_device_time(9);
        let trace = crate::copy_device_trace();
        assert_eq!(trace.len(), 3);
        assert!(matches!(
            trace.first().map(|event| event.kind),
            Some(fn64_runtime::DeviceTraceKind::SpDmaStarted(
                fn64_runtime::SpDmaRequest {
                    direction: fn64_runtime::SpDmaDirection::RspToRdram,
                    ..
                }
            ))
        ));
        assert!(matches!(
            trace.last().map(|event| event.kind),
            Some(fn64_runtime::DeviceTraceKind::SpDmaBusyCleared)
        ));
        let receipt = take_validated_sp_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .expect("fresh completed SP lifecycle must mint one runtime-state prerequisite");
        assert_eq!(receipt.evidence().sp_started, 1);
        assert_eq!(receipt.evidence().sp_committed, 1);
        assert_eq!(receipt.evidence().sp_rsp_to_rdram_committed, 1);
        assert!(receipt.has_valid_evidence_hash());
        assert!(take_validated_sp_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .is_none());
    }


    #[test]
    fn sp_writer_runtime_state_rejects_nonwriting_and_bad_queued_handoff() {
        assert_eq!(
            validate_sp_transition_trace(&sp_test_trace(fn64_runtime::SpDmaDirection::RdramToRsp))
                .unwrap_err(),
            SpWriterRuntimeStateErrorV1::NoRspToRdramCommit
        );
        let first = fn64_runtime::SpDmaRequest {
            direction: fn64_runtime::SpDmaDirection::RspToRdram,
            mem_addr: fn64_runtime::RspMemAddr::from_register(0),
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6000),
            encoded_len: 7,
        };
        let queued = fn64_runtime::SpDmaRequest {
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6010),
            ..first
        };
        let wrong = fn64_runtime::SpDmaRequest {
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6020),
            ..first
        };
        let kinds = [
            fn64_runtime::DeviceTraceKind::SpDmaStarted(first),
            fn64_runtime::DeviceTraceKind::SpDmaQueued(queued),
            fn64_runtime::DeviceTraceKind::SpDmaBytesCommitted(first),
            fn64_runtime::DeviceTraceKind::SpDmaStarted(wrong),
            fn64_runtime::DeviceTraceKind::SpDmaBytesCommitted(wrong),
            fn64_runtime::DeviceTraceKind::SpDmaBusyCleared,
        ];
        let trace = kinds
            .into_iter()
            .enumerate()
            .map(|(sequence, kind)| fn64_runtime::DeviceTraceEvent {
                at: fn64_runtime::EmulatedInstant::new(100 + sequence as u64),
                sequence: sequence as u64,
                kind,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            validate_sp_transition_trace(&trace).unwrap_err(),
            SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder
        );
    }


    #[test]
    fn sp_writer_runtime_state_public_path_rejects_superseded_epoch() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let old = begin_sp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("first SP epoch");
        let current = begin_sp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("replacement SP epoch");
        assert_eq!(
            take_validated_sp_writer_runtime_state_receipt_v1(&old).unwrap_err(),
            SpWriterRuntimeStateErrorV1::TraceEpochMismatch
        );
        assert_eq!(
            take_validated_sp_writer_runtime_state_receipt_v1(&current).unwrap_err(),
            SpWriterRuntimeStateErrorV1::NoSpTransitions
        );
    }


    #[test]
    fn sp_writer_runtime_state_epoch_ids_are_process_unique_across_threads() {
        let mut ids = (0..16)
            .map(|_| std::thread::spawn(next_sp_writer_trace_epoch_id))
            .map(|thread| thread.join().expect("SP epoch mint thread panicked"))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert!(ids.iter().all(|id| *id != 0));
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }


    #[test]
    fn cpu_writer_runtime_state_public_path_owns_fresh_quiescent_store_window() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_cpu_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one CPU-store epoch");
        assert_eq!(
            take_validated_cpu_writer_runtime_state_receipt_v1(&epoch).unwrap_err(),
            CpuWriterRuntimeStateErrorV1::NoCpuStores
        );

        record_executable_and_renderer_write(GuestWriteEvent::Range {
            channel: WriterChannel::CpuInstructionStore,
            physical_offset: 0x6000,
            len: 4,
        });
        assert_eq!(
            take_validated_cpu_writer_runtime_state_receipt_v1(&epoch).unwrap_err(),
            CpuWriterRuntimeStateErrorV1::PendingPhysicalWrites
        );
        let live = with_host(|host| host.canonical_recompiled_program.clone().unwrap());
        let storage = with_host(|host| host.owned_runtime_rdram.as_deref().unwrap().to_vec());
        let view = fn64_runtime::RdramView::from_storage(&storage);
        live.invalidate_pending_physical_writes_with(|physical| {
            view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
        });

        let receipt = take_validated_cpu_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .expect("fresh quiescent CPU store must mint one runtime-state prerequisite");
        assert_eq!(receipt.evidence().cpu_store_count, 1);
        assert_eq!(receipt.evidence().cpu_journal_declaration_count, 0);
        assert!(receipt.has_valid_evidence_hash());
        assert!(take_validated_cpu_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .is_none());
    }


    #[test]
    fn cpu_writer_runtime_state_rejects_superseded_epoch_and_invalid_ranges() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let old = begin_cpu_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("first CPU-store epoch");
        let current = begin_cpu_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("replacement CPU-store epoch");
        assert_eq!(
            take_validated_cpu_writer_runtime_state_receipt_v1(&old).unwrap_err(),
            CpuWriterRuntimeStateErrorV1::TraceEpochMismatch
        );
        record_executable_and_renderer_write(GuestWriteEvent::Range {
            channel: WriterChannel::CpuInstructionStore,
            physical_offset: fn64_cpu_runtime::RDRAM_LEN as u32,
            len: 4,
        });
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        assert_eq!(
            take_validated_cpu_writer_runtime_state_receipt_v1(&current).unwrap_err(),
            CpuWriterRuntimeStateErrorV1::InvalidCpuStoreRange
        );
    }


    #[test]
    fn cpu_writer_runtime_state_epoch_ids_are_process_unique_across_threads() {
        let mut ids = (0..16)
            .map(|_| std::thread::spawn(next_cpu_writer_trace_epoch_id))
            .map(|thread| thread.join().expect("CPU epoch mint thread panicked"))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert!(ids.iter().all(|id| *id != 0));
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }


    #[test]
    fn host_abi_writer_runtime_state_binds_exact_catalog_lifecycle_and_journal() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let mut validated = transaction.commit().unwrap();
        let mut state = CanonicalExecutableMutationStateV1::from_bootstrap(
            validated.receipt().evidence(),
            &validated.storage,
        );
        let target = GuestPc::new(0x8000_1000);
        let host_catalog = issue_abi_host_function_catalog_v1(vec![AbiHostShimBindingV1 {
            target_pc: target.get(),
            shim: AbiHostShimV1::OsCreateMesgQueue,
        }])
        .unwrap();
        let host_catalog_evidence = host_catalog.evidence().clone();
        state.host_abi_writer_trace = Some(HostAbiWriterTraceV1 {
            epoch_id: 41,
            initial_journal_entry_count: 1,
            events: Vec::new(),
        });
        let transaction =
            state.begin_host_transaction(7, target, ExecutionKey::new(INSTALL_BANK, INSTALL_PC));
        unsafe {
            fn64_runtime::RdramPtr::from_storage_ptr(validated.storage.as_mut_ptr()).write_u8(
                fn64_runtime::RdramAddr::from_offset(INSTALL_PC.get() & 0x1fff_ffff),
                0xaa,
            );
        }
        let view = fn64_runtime::RdramView::from_storage(&validated.storage);
        let snapshot = state
            .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
        state.commit_snapshot(
            snapshot,
            vec![GuestWriteEvent::Range {
                channel: WriterChannel::HostAbi,
                physical_offset: INSTALL_PC.get() & 0x1fff_ffff,
                len: 1,
            }],
            Vec::new(),
        );
        state.record_host_abi_boundary(transaction, 1);
        state.finish_host_transaction(transaction);
        let trace = state.host_abi_writer_trace.clone().unwrap();
        let receipt = validate_host_abi_writer_runtime_state_v1(
            [0x11; 32],
            [0x22; 32],
            Some(&host_catalog_evidence),
            production_aot_receipt_for_si_test(),
            true,
            Some(41),
            &validated.storage,
            &state,
            Some(&trace),
        )
        .unwrap();
        let evidence = receipt.evidence();
        assert_eq!(evidence.transactions_started, 1);
        assert_eq!(evidence.transactions_finished, 1);
        assert_eq!(evidence.ordering_boundaries, 1);
        assert_eq!(evidence.host_abi_journal_entry_count, 1);
        assert_eq!(evidence.host_abi_journal_declaration_count, 1);
        assert!(receipt.has_valid_evidence_hash());
    }


    #[test]
    fn host_abi_writer_runtime_state_rejects_call_without_write_and_unknown_target() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let state = CanonicalExecutableMutationStateV1::from_bootstrap(
            validated.receipt().evidence(),
            &validated.storage,
        );
        let target = GuestPc::new(0x8000_1000);
        let host_catalog = issue_abi_host_function_catalog_v1(vec![AbiHostShimBindingV1 {
            target_pc: target.get(),
            shim: AbiHostShimV1::OsCreateMesgQueue,
        }])
        .unwrap();
        let frame = OpenHostMutationTransactionEvidenceV1 {
            transaction_id: 3,
            thread: 5,
            target,
            resume: ExecutionKey::new(INSTALL_BANK, INSTALL_PC),
        };
        let trace = HostAbiWriterTraceV1 {
            epoch_id: 42,
            initial_journal_entry_count: 1,
            events: vec![
                HostAbiWriterTraceEventV1::Started(frame),
                HostAbiWriterTraceEventV1::Boundary {
                    transaction_id: 3,
                    thread: 5,
                    journal_sequences: Vec::new(),
                },
                HostAbiWriterTraceEventV1::Finished {
                    transaction_id: 3,
                    thread: 5,
                },
            ],
        };
        let validate = |trace: &HostAbiWriterTraceV1| {
            validate_host_abi_writer_runtime_state_v1(
                [0x11; 32],
                [0x22; 32],
                Some(host_catalog.evidence()),
                production_aot_receipt_for_si_test(),
                true,
                Some(42),
                &validated.storage,
                &state,
                Some(trace),
            )
            .unwrap_err()
        };
        assert_eq!(
            validate(&trace),
            HostAbiWriterRuntimeStateErrorV1::NoHostAbiWriteCommit
        );
        let mut unknown = trace;
        if let HostAbiWriterTraceEventV1::Started(frame) = &mut unknown.events[0] {
            frame.target = GuestPc::new(0x8000_2000);
        }
        assert_eq!(
            validate(&unknown),
            HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle
        );
    }


    #[test]
    fn host_abi_writer_runtime_state_epoch_ids_are_process_unique_across_threads() {
        let mut ids = (0..16)
            .map(|_| std::thread::spawn(next_host_abi_writer_trace_epoch_id))
            .map(|thread| thread.join().expect("Host ABI epoch mint thread panicked"))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert!(ids.iter().all(|id| *id != 0));
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }


    #[test]
    fn sp_writer_runtime_state_public_epoch_rejects_pending_device_and_abi_rsp_owners() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        with_host(|host| {
            host.rsp_interpreter_state =
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight {
                    owner: crate::task_dispatch::RspInterpreterOwner::RawKick {
                        admission_generation:
                            crate::task_dispatch::RspTaskAdmissionGeneration::first(),
                    },
                };
        });
        assert_eq!(
            begin_sp_writer_runtime_trace_epoch_v1().unwrap_err(),
            SpWriterRuntimeStateErrorV1::PendingAbiSpWork
        );
        with_host(|host| {
            host.rsp_interpreter_state =
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::Reset;
        });
        crate::pi::start_live_rcp_task_with_latency(
            fn64_runtime::RcpTaskCompletionPlan::SpOnly,
            10,
        )
        .unwrap();
        assert_eq!(
            begin_sp_writer_runtime_trace_epoch_v1().unwrap_err(),
            SpWriterRuntimeStateErrorV1::PendingDeviceSpTask
        );
    }


    #[test]
    fn sp_writer_runtime_state_accepts_exact_queued_handoff() {
        let first = fn64_runtime::SpDmaRequest {
            direction: fn64_runtime::SpDmaDirection::RspToRdram,
            mem_addr: fn64_runtime::RspMemAddr::from_register(0),
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6000),
            encoded_len: 7,
        };
        let queued = fn64_runtime::SpDmaRequest {
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6010),
            ..first
        };
        let kinds = [
            fn64_runtime::DeviceTraceKind::SpDmaStarted(first),
            fn64_runtime::DeviceTraceKind::SpDmaQueued(queued),
            fn64_runtime::DeviceTraceKind::SpDmaBytesCommitted(first),
            fn64_runtime::DeviceTraceKind::SpDmaStarted(queued),
            fn64_runtime::DeviceTraceKind::SpDmaBytesCommitted(queued),
            fn64_runtime::DeviceTraceKind::SpDmaBusyCleared,
        ];
        let trace = kinds
            .into_iter()
            .enumerate()
            .map(|(sequence, kind)| fn64_runtime::DeviceTraceEvent {
                at: fn64_runtime::EmulatedInstant::new(100 + sequence as u64),
                sequence: sequence as u64,
                kind,
            })
            .collect::<Vec<_>>();
        let (started, queued, committed, busy_cleared, writes, digest) =
            validate_sp_transition_trace(&trace).unwrap();
        assert_eq!(
            (started, queued, committed, busy_cleared, writes),
            (2, 1, 2, 1, 2)
        );
        assert_ne!(digest, [0; 32]);
    }


    #[test]
    fn si_writer_runtime_state_prerequisite_binds_quiescent_trace_and_journal() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let state = CanonicalExecutableMutationStateV1::from_bootstrap(
            validated.receipt().evidence(),
            &validated.storage,
        );
        let receipt = validate_si_writer_runtime_state_v1(
            [0x11; 32],
            [0x22; 32],
            Some([0x33; 32]),
            production_aot_receipt_for_si_test(),
            true,
            &validated.storage,
            &state,
            &si_test_trace(fn64_runtime::SiDmaKind::PifToDram),
            false,
            false,
        )
        .unwrap();
        let evidence = receipt.evidence();
        assert_eq!(evidence.schema, SI_WRITER_RUNTIME_STATE_SCHEMA_V1);
        assert_eq!(evidence.si_started, 1);
        assert_eq!(evidence.si_committed, 1);
        assert_eq!(evidence.si_pif_to_dram_committed, 1);
        assert_eq!(evidence.journal_entry_count, 1);
        assert_eq!(evidence.si_journal_declaration_count, 0);
        assert_eq!(evidence.journal_root_sha256, state.journal_root_sha256);
        assert!(receipt.has_valid_evidence_hash());
    }


    #[test]
    fn si_writer_runtime_state_rejects_missing_authority_and_nonquiescence() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let state = CanonicalExecutableMutationStateV1::from_bootstrap(
            validated.receipt().evidence(),
            &validated.storage,
        );
        let validate = |host, build, pending_device, pending_abi| {
            validate_si_writer_runtime_state_v1(
                [0x11; 32],
                [0x22; 32],
                host,
                build,
                true,
                &validated.storage,
                &state,
                &si_test_trace(fn64_runtime::SiDmaKind::PifToDram),
                pending_device,
                pending_abi,
            )
            .unwrap_err()
        };
        assert_eq!(
            validate(None, production_aot_receipt_for_si_test(), false, false),
            SiWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority
        );
        assert_eq!(
            validate(
                Some([0x33; 32]),
                StaticExecutionBuildReceipt {
                    schema: 1,
                    aot_runtime: true,
                    production_aot: false,
                    dev_interpreter: true,
                },
                false,
                false,
            ),
            SiWriterRuntimeStateErrorV1::NonProductionAotBuild
        );
        assert_eq!(
            validate(
                Some([0x33; 32]),
                production_aot_receipt_for_si_test(),
                true,
                false,
            ),
            SiWriterRuntimeStateErrorV1::PendingDeviceSi
        );
        assert_eq!(
            validate(
                Some([0x33; 32]),
                production_aot_receipt_for_si_test(),
                false,
                true,
            ),
            SiWriterRuntimeStateErrorV1::PendingAbiSi
        );
    }


    #[test]
    fn si_writer_runtime_state_rejects_incomplete_or_nonwriting_trace() {
        let complete = si_test_trace(fn64_runtime::SiDmaKind::PifToDram);
        assert_eq!(
            validate_si_transition_trace(&complete[..complete.len() - 1]).unwrap_err(),
            SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder
        );
        assert_eq!(
            validate_si_transition_trace(&si_test_trace(fn64_runtime::SiDmaKind::DramToPif))
                .unwrap_err(),
            SiWriterRuntimeStateErrorV1::NoPifToDramCommit
        );
        let mut drifted = complete;
        if let fn64_runtime::DeviceTraceKind::SiBytesCommitted(ref mut request) = drifted[1].kind {
            request.dram_addr = fn64_runtime::RdramAddr::from_offset(0x7040);
        }
        assert_eq!(
            validate_si_transition_trace(&drifted).unwrap_err(),
            SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder
        );
        let mut nonmonotonic = si_test_trace(fn64_runtime::SiDmaKind::PifToDram);
        nonmonotonic[3].at = fn64_runtime::EmulatedInstant::new(99);
        assert_eq!(
            validate_si_transition_trace(&nonmonotonic).unwrap_err(),
            SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder
        );
        let mut sequence_regression = si_test_trace(fn64_runtime::SiDmaKind::PifToDram);
        sequence_regression[3].at = fn64_runtime::EmulatedInstant::new(200);
        sequence_regression[3].sequence = 1;
        assert_eq!(
            validate_si_transition_trace(&sequence_regression).unwrap_err(),
            SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder
        );
    }


    #[test]
    fn si_writer_runtime_state_receipt_has_one_successful_take() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let CatalogGenerationInstallV1 {
            mut resolver,
            generations,
        } = install;
        let AbiHostFunctionCatalogV1 { catalog, evidence } =
            issue_abi_host_function_catalog_v1(Vec::new()).unwrap();
        resolver.host_functions = catalog;
        resolver.evidence.abi_host_catalog = Some(evidence);
        resolver.evidence.build_receipt = production_aot_receipt_for_si_test();
        let watched_ranges = validated.receipt().evidence().watched_ranges.clone();
        let writer_program_model_sha256 =
            canonical_writer_program_model_sha256(&resolver, Some(&generations), &watched_ranges);
        let state = CanonicalExecutableMutationStateV1::from_bootstrap(
            validated.receipt().evidence(),
            &validated.storage,
        );
        let live = CanonicalLiveBlockProgramV1 {
            install: Rc::new(resolver),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_units: Rc::new(RefCell::new(None)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_withheld_static_key: Rc::new(Cell::new(None)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_execution_aggregates: Rc::new(RefCell::new(BTreeMap::new())),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_identity_activations: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_identity_charged_instructions: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_identity_unsupported_exits: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_attempted_entry_activations: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_attempted_entry_charged_instructions: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_attempted_entry_unsupported_exits: Rc::new(Cell::new(0)),
            canonical_charged_instructions: Rc::new(Cell::new(0)),
            canonical_instruction_limit: Rc::new(Cell::new(None)),
            thread_publications: Rc::new(RefCell::new(BTreeMap::new())),
            generations: Some(Rc::new(RefCell::new(generations))),
            mutation_state: Some(Rc::new(RefCell::new(state))),
            bootstrap_evidence: Some(validated.receipt().evidence().clone()),
            writer_program_model_sha256,
            bootstrap_writer_completion: Rc::new(RefCell::new(None)),
            cpu_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            cpu_writer_trace_epoch_id: Rc::new(Cell::new(None)),
            pi_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            pi_writer_trace_epoch_id: Rc::new(Cell::new(None)),
            si_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            sp_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            sp_writer_trace_epoch_id: Rc::new(Cell::new(None)),
            host_abi_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            rsp_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            rsp_writer_trace_epoch_id: Rc::new(Cell::new(None)),
            rdp_renderer_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            rdp_renderer_writer_trace_epoch_id: Rc::new(Cell::new(None)),
        };
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let trace = si_test_trace(fn64_runtime::SiDmaKind::PifToDram);
        assert!(live
            .take_si_writer_runtime_state(&validated.storage, true, &trace, false, false,)
            .unwrap()
            .is_some());
        assert!(live
            .take_si_writer_runtime_state(&validated.storage, true, &trace, false, false,)
            .unwrap()
            .is_none());
    }


    #[test]
    fn bootstrap_import_rejects_wrong_entry_image_and_conflicting_publication() {
        let install = bootstrap_test_install(0x2402_0001);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&0x2402_0002u32.to_be_bytes());
        rom[0x24..0x28].copy_from_slice(&0x2402_0003u32.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        assert!(matches!(
            transaction.publish_resident_rom_image(0x24, 0x8000_7000, 4),
            Err(BootstrapImportErrorV1::ConflictingPublication { .. })
        ));
        assert!(matches!(
            transaction.commit(),
            Err(BootstrapImportErrorV1::InitialEntryImageMismatch {
                expected: 0x2402_0001,
                actual: 0x2402_0002,
                ..
            })
        ));
    }


    #[test]
    fn bootstrap_import_rejects_a_wrong_non_entry_static_bank() {
        let entry_word = 0x2402_0001;
        let static_word = 0x2403_0002;
        let physical_word = 0x2404_0003;
        let install =
            bootstrap_test_install_with_additional_banks(entry_word, static_word, physical_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&entry_word.to_be_bytes());
        rom[0x24..0x28].copy_from_slice(&(static_word + 1).to_be_bytes());
        rom[0x28..0x2c].copy_from_slice(&physical_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x24, 0x8000_8000, 4)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x28, 0x8000_9000, 4)
            .unwrap();

        assert!(matches!(
            transaction.commit(),
            Err(BootstrapImportErrorV1::StaticProgramImageMismatch {
                bank,
                pc,
                expected,
                actual,
            }) if bank == BankId::new(0xb008)
                && pc == GuestPc::new(0x8000_8000)
                && expected == static_word
                && actual == static_word + 1
        ));
    }


    #[test]
    fn bootstrap_import_rejects_a_wrong_physical_bank() {
        let entry_word = 0x2402_0001;
        let static_word = 0x2403_0002;
        let physical_word = 0x2404_0003;
        let install =
            bootstrap_test_install_with_additional_banks(entry_word, static_word, physical_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&entry_word.to_be_bytes());
        rom[0x24..0x28].copy_from_slice(&static_word.to_be_bytes());
        rom[0x28..0x2c].copy_from_slice(&(physical_word + 1).to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x24, 0x8000_8000, 4)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x28, 0x8000_9000, 4)
            .unwrap();

        assert!(matches!(
            transaction.commit(),
            Err(BootstrapImportErrorV1::PhysicalProgramImageMismatch {
                bank,
                physical_address: 0x9000,
                expected,
                actual,
            }) if bank == BankId::new(0xb009)
                && expected == physical_word
                && actual == physical_word + 1
        ));
    }


    #[test]
    fn bootstrap_import_does_not_expect_future_bytes_for_a_reserved_generation_bank() {
        let entry_word = 0x2402_0001;
        let future_word = 0x3c1a_8003;
        let future_bank = BankId::new(0xb00a);
        let install = bootstrap_test_install_with_generation(entry_word, future_word);
        assert!(install.generations.contains_reserved_bank(future_bank));

        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&entry_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        assert!(transaction
            .commit()
            .unwrap()
            .receipt()
            .evidence()
            .initial_generations
            .is_empty());
    }


    #[test]
    fn bootstrap_import_binds_zero_or_exact_generation_images() {
        let entry_word = 0x2402_0001;
        let generation_word = 0x2403_0002;
        let install = bootstrap_test_install_with_generation(entry_word, generation_word);

        let mut zero_rom = vec![0; 0x40];
        zero_rom[0x20..0x24].copy_from_slice(&entry_word.to_be_bytes());
        let mut zero = install
            .begin_bootstrap_import_v1(
                &zero_rom,
                bootstrap_test_rdram_len(),
                fn64_runtime::TvType::Ntsc,
            )
            .unwrap();
        zero.publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        assert!(zero
            .commit()
            .unwrap()
            .receipt()
            .evidence()
            .initial_generations
            .is_empty());

        let mut exact_rom = zero_rom.clone();
        exact_rom[0x24..0x28].copy_from_slice(&generation_word.to_be_bytes());
        let mut exact = install
            .begin_bootstrap_import_v1(
                &exact_rom,
                bootstrap_test_rdram_len(),
                fn64_runtime::TvType::Ntsc,
            )
            .unwrap();
        exact
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        exact
            .publish_resident_rom_image(0x24, 0x8000_a000, 4)
            .unwrap();
        assert_eq!(
            exact
                .commit()
                .unwrap()
                .receipt()
                .evidence()
                .initial_generations,
            [GenerationId::new(0xaaa)]
        );

        let unknown_word = generation_word + 1;
        let mut unknown_rom = zero_rom;
        unknown_rom[0x24..0x28].copy_from_slice(&unknown_word.to_be_bytes());
        let mut unknown = install
            .begin_bootstrap_import_v1(
                &unknown_rom,
                bootstrap_test_rdram_len(),
                fn64_runtime::TvType::Ntsc,
            )
            .unwrap();
        unknown
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        unknown
            .publish_resident_rom_image(0x24, 0x8000_a000, 4)
            .unwrap();
        assert!(matches!(
            unknown.commit(),
            Err(BootstrapImportErrorV1::UnrecognizedInitialGenerationImage {
                physical_address: 0xa000,
                actual: 0x24,
            })
        ));
    }


    #[test]
    fn bootstrap_import_exact_duplicate_is_canonicalized() {
        let install = bootstrap_test_install(0x2402_0001);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&0x2402_0001u32.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        assert_eq!(
            transaction
                .commit()
                .unwrap()
                .receipt()
                .evidence()
                .publications
                .len(),
            1
        );
    }
