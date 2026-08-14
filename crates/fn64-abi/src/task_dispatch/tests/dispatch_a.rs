use super::*;

    #[test]
    fn diagnostic_graphics_skip_advances_the_sp_then_dp_scheduler() {
        let full_sync =
            diagnostic_graphics_dp_full_sync(GraphicsTaskExecutionPolicy::DiagnosticSkip)
                .expect("diagnostic skip must publish its synthetic completion");
        assert_eq!(full_sync, fn64_render::DpFullSyncStatus::Reached);
        assert_eq!(
            rcp_completion_plan(full_sync, "diagnostic graphics skip"),
            fn64_runtime::RcpTaskCompletionPlan::SpThenDpFullSync
        );
        assert_eq!(
            diagnostic_graphics_dp_full_sync(GraphicsTaskExecutionPolicy::LleAccuracy),
            None
        );
        assert_eq!(
            diagnostic_graphics_dp_full_sync(GraphicsTaskExecutionPolicy::HleOptimized),
            None
        );
    }


    #[test]
    fn task_admission_generation_public_constructor_preserves_nonzero_value() {
        let generation = RspTaskAdmissionGeneration::new(NonZeroU64::new(7).unwrap());
        assert_eq!(generation.get(), 7);
    }


    #[test]
    fn rsp_writer_trace_binds_exact_epoch_owner_and_commit_order() {
        let first_owner = RspInterpreterOwner::task(
            0x40,
            RspTaskAdmissionGeneration::new(NonZeroU64::new(7).unwrap()),
        );
        let second_owner = RspInterpreterOwner::RawKick {
            admission_generation: RspTaskAdmissionGeneration::new(NonZeroU64::new(8).unwrap()),
        };
        begin_rsp_writer_trace_v1(91);
        record_rsp_writer_commits_v1(
            RspWriterCommitSourceV1::Interpreter { owner: first_owner },
            &[(0x100, 0x110), (0x300, 0x304)],
        );
        record_rsp_writer_commits_v1(
            RspWriterCommitSourceV1::Interpreter {
                owner: second_owner,
            },
            &[(0x200, 0x208)],
        );

        assert_eq!(rsp_writer_trace_snapshot_v1(90), None);
        assert_eq!(
            rsp_writer_trace_snapshot_v1(91).unwrap(),
            RspWriterTraceSnapshotV1 {
                commits: vec![
                    RspWriterCommitObservationV1 {
                        source: RspWriterCommitSourceV1::Interpreter { owner: first_owner },
                        physical_start: 0x100,
                        physical_end: 0x110,
                    },
                    RspWriterCommitObservationV1 {
                        source: RspWriterCommitSourceV1::Interpreter { owner: first_owner },
                        physical_start: 0x300,
                        physical_end: 0x304,
                    },
                    RspWriterCommitObservationV1 {
                        source: RspWriterCommitSourceV1::Interpreter {
                            owner: second_owner,
                        },
                        physical_start: 0x200,
                        physical_end: 0x208,
                    }
                ],
                hle_publications: Vec::new(),
                rejected_journal_sequences: Vec::new(),
            }
        );
        assert!(!finish_rsp_writer_trace_v1(90));
        assert!(finish_rsp_writer_trace_v1(91));
        assert_eq!(rsp_writer_trace_snapshot_v1(91), None);
    }


    #[test]
    fn rsp_writer_trace_rearm_supersedes_older_observations() {
        let owner = RspInterpreterOwner::task(
            0x80,
            RspTaskAdmissionGeneration::new(NonZeroU64::new(1).unwrap()),
        );
        begin_rsp_writer_trace_v1(11);
        finish_translated_audio_hle_publication_v1(
            RspWriterCommitSourceV1::TranslatedAudioHle { owner },
            vec![3],
            true,
        );

        begin_rsp_writer_trace_v1(12);
        assert_eq!(rsp_writer_trace_snapshot_v1(11), None);
        assert_eq!(
            rsp_writer_trace_snapshot_v1(12),
            Some(RspWriterTraceSnapshotV1 {
                commits: Vec::new(),
                hle_publications: Vec::new(),
                rejected_journal_sequences: Vec::new(),
            })
        );
        assert!(finish_rsp_writer_trace_v1(12));
    }


    #[test]
    fn rsp_interpreter_owner_preserves_core_state_and_overlays_device_latches() {
        with_host(|host| *host = HostState::default());
        let mut first_rdram = vec![0u8; 0x1000];
        let mut first = fn64_audio::rsp::runtime::RspMachine::new(&mut first_rdram);
        let first_task = RdramAddr::from_offset(0x40);
        install_running_task_lineage(first_task, RspTaskAdmissionGeneration::first());
        begin_rsp_interpreter_phase(task_interpreter_owner(first_task), &mut first);
        first.ctx.r[3] = 0x1122_3344;
        first.ctx.jump_target = 0x1550;
        first.ctx.rsp.regs.r[7] = [1, -2, 3, -4, 5, -6, 7, -8];
        first.ctx.rsp.acc.set(2, -0x1234_5678);
        first.ctx.rsp.flags.vco = 0x55aa;
        first.ctx.rsp.flags.vcc = 0xaa55;
        first.ctx.rsp.flags.vce = 0x69;
        first.ctx.rsp.div_in = 0x4567;
        first.ctx.rsp.div_in_loaded = true;
        first.ctx.rsp.div_out = 0x89ab;
        first.ctx.steps = 91;
        let committed = first.snapshot_architectural_state();
        commit_rsp_interpreter_phase(task_interpreter_owner(first_task), committed.clone());

        let fabric = with_host(|host| {
            let mut state = host.device_fabric.rsp_execution_state();
            state.sp_status = 0x0000_0403;
            state.sp_semaphore = true;
            state.sp_dma_mem_addr = fn64_runtime::RspMemAddr::from_register(0x1230);
            state.sp_dma_dram_addr = RdramAddr::from_offset(0x456788);
            state.sp_dma_read_length = 0x0102_0304;
            state.sp_dma_write_length = 0x1112_1314;
            state.dpc_start = 0x100;
            state.dpc_end = 0x180;
            state.dpc_current = 0x140;
            state.dpc_status = 0x21;
            state.dpc_clock = 0x3132_3334;
            state.dpc_busy = 0x4142_4344;
            state.dpc_pipe_busy = 0x5152_5354;
            state.dpc_tmem_busy = 0x6162_6364;
            host.device_fabric
                .commit_complete_rsp_execution_state(state)
                .unwrap();
            host.device_fabric.rsp_execution_state()
        });

        let mut second_rdram = vec![0u8; 0x1000];
        let mut second = fn64_audio::rsp::runtime::RspMachine::new(&mut second_rdram);
        let second_task = RdramAddr::from_offset(0x80);
        install_running_task_lineage(
            second_task,
            RspTaskAdmissionGeneration::new(NonZeroU64::new(2).unwrap()),
        );
        begin_rsp_interpreter_phase(task_interpreter_owner(second_task), &mut second);
        let restored = second.snapshot_architectural_state();
        assert_eq!(restored.gprs(), committed.gprs());
        assert_eq!(restored.jump_target(), committed.jump_target());
        assert_eq!(restored.vu(), committed.vu());
        assert_eq!(
            second.ctx.steps, 0,
            "diagnostics must not cross task boundaries"
        );
        assert_eq!(restored.sp_semaphore(), fabric.sp_semaphore);
        assert_eq!(
            restored.dma_mem_address(),
            u32::from(fabric.sp_dma_mem_addr.get())
        );
        assert_eq!(
            restored.dma_dram_address(),
            fabric.sp_dma_dram_addr.offset()
        );
        assert_eq!(restored.dma_read_length(), fabric.sp_dma_read_length);
        assert_eq!(restored.dma_write_length(), fabric.sp_dma_write_length);
        assert_eq!(restored.dp_start(), fabric.dpc_start);
        assert_eq!(restored.dp_end(), fabric.dpc_end);
        assert_eq!(restored.dp_current(), fabric.dpc_current);
        assert_eq!(restored.dp_status(), fabric.dpc_status);
        assert_eq!(restored.dp_clock(), fabric.dpc_clock);
        assert_eq!(restored.dp_busy(), fabric.dpc_busy);
        assert_eq!(restored.dp_pipe_busy(), fabric.dpc_pipe_busy);
        assert_eq!(restored.dp_tmem_busy(), fabric.dpc_tmem_busy);
        assert_eq!(restored.dp_submissions(), &[]);
        assert_eq!(
            restored.sp_status(),
            fabric.sp_status & !(fn64_runtime::SP_STATUS_HALT | fn64_runtime::SP_STATUS_BROKE)
        );
        commit_rsp_interpreter_phase(task_interpreter_owner(second_task), restored);
    }


    #[test]
    fn whole_audio_capture_matches_begin_state_and_mutates_only_its_owner() {
        let mut rdram = Vec::new();
        let header = boot_overlay_audio_header();
        let (task_addr, admission_generation) = prepare_audio_capture_task(&mut rdram, header);

        let mut prior_storage = [];
        let mut prior_machine = fn64_audio::rsp::runtime::RspMachine::new(&mut prior_storage);
        prior_machine.ctx.r[3] = 0x1122_3344;
        prior_machine.ctx.rsp.regs.r[7] = [1, -2, 3, -4, 5, -6, 7, -8];
        prior_machine.ctx.rsp.flags.vco = 0x55aa;
        let prior = prior_machine.snapshot_architectural_state();
        with_host(|host| {
            host.rsp_interpreter_state = RspInterpreterStateEvidenceSnapshot::Exact(prior.clone());
            let mut fabric = host.device_fabric.rsp_execution_state();
            fabric.sp_status = fn64_runtime::SP_STATUS_HALT
                | fn64_runtime::SP_STATUS_BROKE
                | fn64_runtime::SP_STATUS_SIGNAL_0;
            fabric.sp_semaphore = true;
            fabric.sp_dma_mem_addr = fn64_runtime::RspMemAddr::from_register(0x1230);
            fabric.sp_dma_dram_addr = RdramAddr::from_offset(0x456780);
            host.device_fabric
                .commit_complete_rsp_execution_state(fabric)
                .unwrap();
        });

        let mut expected_storage = [];
        let mut expected_machine = fn64_audio::rsp::runtime::RspMachine::new(&mut expected_storage);
        begin_rsp_interpreter_phase(task_interpreter_owner(task_addr), &mut expected_machine);
        let expected_state = expected_machine.snapshot_state();
        with_host(|host| {
            host.rsp_interpreter_state = RspInterpreterStateEvidenceSnapshot::Exact(prior);
        });
        let before = crate::host_evidence_snapshot();

        let captured =
            unsafe { capture_audio_whole_task_input(rdram.as_mut_ptr(), task_addr, header) };

        assert_eq!(captured.owner.task_addr, task_addr);
        assert_eq!(captured.owner.admission_generation, admission_generation);
        assert_eq!(captured.input.initial_machine_state(), &expected_state);
        assert_eq!(captured.input.initial_pc_low12(), 0);
        assert_eq!(captured.input.rdram_storage(), &rdram[..]);
        assert_eq!(
            captured.input.rsp_memory(),
            &with_host(|host| host.device_fabric.rsp_memory().snapshot())
        );
        let mut expected_after = before;
        expected_after.rsp_interpreter_state = RspInterpreterStateEvidenceSnapshot::InFlight {
            owner: RspInterpreterOwner::task(task_addr.offset(), admission_generation),
        };
        assert_eq!(crate::host_evidence_snapshot(), expected_after);
    }


    #[test]
    fn whole_audio_capture_rejects_same_address_stale_inflight_generation() {
        let mut rdram = Vec::new();
        let header = boot_overlay_audio_header();
        let (task_addr, first_generation) = prepare_audio_capture_task(&mut rdram, header);
        let second_generation =
            RspTaskAdmissionGeneration::new(NonZeroU64::new(first_generation.get() + 1).unwrap());
        with_host(|host| {
            host.rsp_task_lineages
                .get_mut(&task_addr.offset())
                .unwrap()
                .admission_generation = second_generation;
            host.rsp_interpreter_state = RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: RspInterpreterOwner::task(task_addr.offset(), first_generation),
            };
        });
        let before = crate::host_evidence_snapshot();

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            capture_audio_whole_task_input(rdram.as_mut_ptr(), task_addr, header)
        }));

        let panic = match rejected {
            Err(panic) => panic,
            Ok(_) => panic!("stale same-address owner unexpectedly captured"),
        };
        assert!(panic_message(panic.as_ref()).contains("left a pending interpreter continuation"));
        assert_eq!(crate::host_evidence_snapshot(), before);
    }


    #[test]
    fn whole_audio_direct_imem_rejection_retains_acquired_owner() {
        let mut rdram = Vec::new();
        let header = OsTaskHeader {
            ucode: 0xa000_0100,
            ..boot_overlay_audio_header()
        };
        let (task_addr, admission_generation) = prepare_audio_capture_task(&mut rdram, header);
        let before_rdram = rdram.clone();
        let before_rsp = with_host(|host| host.device_fabric.rsp_memory().snapshot());

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            capture_audio_whole_task_input(rdram.as_mut_ptr(), task_addr, header)
        }));

        let panic = match rejected {
            Err(panic) => panic,
            Ok(_) => panic!("direct-IMEM input unexpectedly captured"),
        };
        assert!(panic_message(panic.as_ref()).contains("DirectImemUnsupported"));
        assert_eq!(rdram, before_rdram);
        assert_eq!(
            with_host(|host| host.device_fabric.rsp_memory().snapshot()),
            before_rsp
        );
        assert_eq!(
            crate::host_evidence_snapshot().rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: RspInterpreterOwner::task(task_addr.offset(), admission_generation),
            }
        );
    }


    #[test]
    fn whole_audio_static_alias_rejection_retains_acquired_owner() {
        let mut rdram = Vec::new();
        let header = OsTaskHeader {
            ucode: 0xa000_0000 | fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u32,
            ..boot_overlay_audio_header()
        };
        let (task_addr, admission_generation) = prepare_audio_capture_task(&mut rdram, header);

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            capture_audio_whole_task_input(rdram.as_mut_ptr(), task_addr, header)
        }));

        let panic = match rejected {
            Err(panic) => panic,
            Ok(_) => panic!("static-alias input unexpectedly captured"),
        };
        assert!(panic_message(panic.as_ref()).contains("StaticAliasNotAllowed"));
        assert_eq!(
            crate::host_evidence_snapshot().rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: RspInterpreterOwner::task(task_addr.offset(), admission_generation),
            }
        );
    }


    #[test]
    fn whole_audio_wrong_registered_pointer_rejects_before_physical_read() {
        let mut rdram = Vec::new();
        let header = boot_overlay_audio_header();
        let (task_addr, admission_generation) = prepare_audio_capture_task(&mut rdram, header);
        let before_rdram = rdram.clone();
        let (before_device, before_rsp) = with_host(|host| {
            (
                host.device_fabric.rsp_execution_state(),
                host.device_fabric.rsp_memory().snapshot(),
            )
        });
        let unreadable_wrong_pointer = std::ptr::NonNull::<u8>::dangling().as_ptr();

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            capture_audio_whole_task_input(unreadable_wrong_pointer, task_addr, header)
        }));

        let panic = match rejected {
            Err(panic) => panic,
            Ok(_) => panic!("wrong registered RDRAM pointer unexpectedly captured"),
        };
        assert!(panic_message(panic.as_ref())
            .contains("must use the registered complete physical RDRAM allocation"));
        assert_eq!(rdram, before_rdram);
        with_host(|host| {
            assert_eq!(host.device_fabric.rsp_execution_state(), before_device);
            assert_eq!(host.device_fabric.rsp_memory().snapshot(), before_rsp);
        });
        assert_eq!(
            crate::host_evidence_snapshot().rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: RspInterpreterOwner::task(task_addr.offset(), admission_generation),
            }
        );
    }


    #[test]
    fn whole_audio_short_registered_allocation_rejects_before_physical_read() {
        let mut rdram = Vec::new();
        let header = boot_overlay_audio_header();
        let (task_addr, admission_generation) = prepare_audio_capture_task(&mut rdram, header);
        with_host(|host| host.runtime_rdram_len = 0x1000);
        let before_rdram = rdram.clone();
        let (before_device, before_rsp) = with_host(|host| {
            (
                host.device_fabric.rsp_execution_state(),
                host.device_fabric.rsp_memory().snapshot(),
            )
        });

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            capture_audio_whole_task_input(rdram.as_mut_ptr(), task_addr, header)
        }));

        let panic = match rejected {
            Err(panic) => panic,
            Ok(_) => panic!("short registered RDRAM allocation unexpectedly captured"),
        };
        assert!(panic_message(panic.as_ref())
            .contains("must use the registered complete physical RDRAM allocation"));
        assert_eq!(rdram, before_rdram);
        with_host(|host| {
            assert_eq!(host.device_fabric.rsp_execution_state(), before_device);
            assert_eq!(host.device_fabric.rsp_memory().snapshot(), before_rsp);
        });
        assert_eq!(
            crate::host_evidence_snapshot().rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: RspInterpreterOwner::task(task_addr.offset(), admission_generation),
            }
        );
    }


    #[test]
    fn rsp_interpreter_owner_rejects_pending_cross_task_continuation() {
        with_host(|host| *host = HostState::default());
        let mut source_rdram = vec![0u8; 0x1000];
        let mut source = fn64_audio::rsp::runtime::RspMachine::new(&mut source_rdram);
        source.ctx.resume_address = 0x1180;
        with_host(|host| {
            host.rsp_interpreter_state =
                RspInterpreterStateEvidenceSnapshot::Exact(source.snapshot_architectural_state());
        });
        let mut target_rdram = vec![0u8; 0x1000];
        let mut target = fn64_audio::rsp::runtime::RspMachine::new(&mut target_rdram);
        install_running_task_lineage(
            RdramAddr::from_offset(0x90),
            RspTaskAdmissionGeneration::first(),
        );
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            begin_rsp_interpreter_phase(
                task_interpreter_owner(RdramAddr::from_offset(0x90)),
                &mut target,
            );
        }))
        .expect_err("pending overlay continuation must not become a fresh task");
        assert!(panic_message(panic.as_ref()).contains("pending overlay resume address"));
        assert!(matches!(
            crate::host_evidence_snapshot().rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::Exact(_)
        ));
    }


    #[test]
    fn rom_install_resets_rsp_interpreter_owner() {
        with_host(|host| *host = HostState::default());
        let mut rdram = vec![0u8; 0x1000];
        let mut machine = fn64_audio::rsp::runtime::RspMachine::new(&mut rdram);
        let task = RdramAddr::from_offset(0x40);
        install_running_task_lineage(task, RspTaskAdmissionGeneration::first());
        begin_rsp_interpreter_phase(task_interpreter_owner(task), &mut machine);
        machine.ctx.r[9] = 0xfeed_beef;
        commit_rsp_interpreter_phase(
            task_interpreter_owner(task),
            machine.snapshot_architectural_state(),
        );
        assert!(matches!(
            crate::host_evidence_snapshot().rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::Exact(_)
        ));

        crate::load_rom(vec![0x12, 0x34]);
        assert_eq!(
            crate::host_evidence_snapshot().rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::Reset
        );
    }


    #[test]
    fn direct_imem_hle_cannot_leave_prior_state_labeled_exact() {
        with_host(|host| *host = HostState::default());
        let mut rdram = vec![0u8; 0x1000];
        let mut machine = fn64_audio::rsp::runtime::RspMachine::new(&mut rdram);
        with_host(|host| {
            host.rsp_interpreter_state =
                RspInterpreterStateEvidenceSnapshot::Exact(machine.snapshot_architectural_state());
        });

        let task = RdramAddr::from_offset(0x88);
        install_running_task_lineage(task, RspTaskAdmissionGeneration::first());
        begin_rsp_interpreter_phase(task_interpreter_owner(task), &mut machine);
        commit_rsp_hle_compatibility(task, None);
        assert_eq!(
            crate::host_evidence_snapshot().rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable {
                owner: RspInterpreterOwner::task(0x88, RspTaskAdmissionGeneration::first()),
            }
        );

        let mut next_rdram = vec![0u8; 0x1000];
        let mut next = fn64_audio::rsp::runtime::RspMachine::new(&mut next_rdram);
        install_running_task_lineage(
            RdramAddr::from_offset(0x90),
            RspTaskAdmissionGeneration::new(NonZeroU64::new(2).unwrap()),
        );
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            begin_rsp_interpreter_phase(
                task_interpreter_owner(RdramAddr::from_offset(0x90)),
                &mut next,
            );
        }))
        .expect_err("unavailable direct-IMEM HLE state must not reuse a stale exact snapshot");
        assert!(panic_message(panic.as_ref()).contains("terminal scalar/VU state is unavailable"));
    }

    #[test]
    fn verified_audio_patches_use_logical_guest_byte_order() {
        let patches = fn64_audio::hle_outcome::CanonicalRdramPatches::new(vec![
            fn64_audio::hle_outcome::RdramPatch::new(1, vec![0x11, 0x22, 0x33, 0x44, 0x55])
                .unwrap(),
        ])
        .unwrap();
        let mut storage = vec![0; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];

        let writes = apply_verified_audio_rdram_patches(&mut storage, &patches);

        assert_eq!(writes, vec![(1, 6)]);
        assert_eq!(&storage[..8], &[0x33, 0x22, 0x11, 0, 0, 0, 0x55, 0x44]);
        let view = fn64_runtime::RdramView::from_storage(&storage);
        let mut logical = [0; 5];
        view.copy_logical_bytes(RdramAddr::from_offset(1), &mut logical);
        assert_eq!(logical, [0x11, 0x22, 0x33, 0x44, 0x55]);
    }


    #[test]
    fn verified_audio_rsp_mapping_covers_every_runtime_register() {
        let mut storage = vec![0; 0x4000];
        let mut machine = fn64_audio::rsp::runtime::RspMachine::new(&mut storage);
        machine.set_sp_status_raw(
            fn64_runtime::SP_STATUS_HALT | fn64_runtime::SP_STATUS_BROKE | (1 << 10),
        );
        machine.set_dma_dram(0x100);
        machine.set_dma_mem(0x180);
        let _ = machine.write_cp0(2, 7);
        machine.set_dma_dram(0x200);
        machine.set_dma_mem(0x280);
        let _ = machine.write_cp0(3, 15);
        let _ = machine.read_cp0(7);
        let _ = machine.write_cp0(11, 1 << 1);
        let _ = machine.write_cp0(8, 0x100);
        let _ = machine.write_cp0(9, 0x108);
        let _ = machine.write_cp0(9, 0x110);
        let complete = machine.snapshot_state();
        let architectural = complete.architectural_state();

        let mapped = verified_rsp_execution_state(&complete, 0x0abc);

        assert_eq!(mapped.pc, 0x0abc);
        assert_eq!(mapped.sp_status, architectural.sp_status());
        assert_eq!(mapped.sp_semaphore, architectural.sp_semaphore());
        assert_eq!(
            mapped.sp_dma_mem_addr,
            fn64_runtime::RspMemAddr::from_register(architectural.dma_mem_address())
        );
        assert_eq!(
            mapped.sp_dma_dram_addr,
            RdramAddr::from_offset(architectural.dma_dram_address() & 0x00ff_ffff)
        );
        assert_eq!(mapped.sp_dma_read_length, architectural.dma_read_length());
        assert_eq!(mapped.sp_dma_write_length, architectural.dma_write_length());
        assert_eq!(mapped.dpc_start, architectural.dp_start());
        assert_eq!(mapped.dpc_end, architectural.dp_end());
        assert_eq!(mapped.dpc_current, architectural.dp_current());
        assert_eq!(mapped.dpc_status, architectural.dp_status());
        assert_eq!(mapped.dpc_clock, architectural.dp_clock());
        assert_eq!(mapped.dpc_busy, architectural.dp_busy());
        assert_eq!(mapped.dpc_pipe_busy, architectural.dp_pipe_busy());
        assert_eq!(mapped.dpc_tmem_busy, architectural.dp_tmem_busy());
        assert_eq!(mapped.dpc_start, 0x100);
        assert_eq!(mapped.dpc_end, 0x110);
        assert_eq!(mapped.dpc_current, 0x110);
    }


    #[test]
    fn verified_audio_rsp_memory_restore_preserves_exact_generation() {
        with_host(|host| *host = HostState::default());
        let mut expected = fn64_runtime::RspMemory::new();
        expected
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0x20),
                &[0x12, 0x34],
            )
            .unwrap();
        expected
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0x30),
                &[0x56, 0x78],
            )
            .unwrap();
        expected
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0x40),
                &[0x9a, 0xbc],
            )
            .unwrap();
        let expected_snapshot = expected.snapshot();
        let state = with_host(|host| host.device_fabric.rsp_execution_state());

        with_host(|host| {
            host.device_fabric
                .commit_complete_rsp_execution_state(state)
                .unwrap();
            host.device_fabric
                .rsp_memory_mut()
                .restore(expected_snapshot.clone());
        });

        with_host(|host| {
            assert_eq!(
                host.device_fabric.rsp_memory().snapshot(),
                expected_snapshot
            );
            assert_eq!(host.device_fabric.rsp_memory().imem_generation(), 2);
        });
    }


    #[test]
    fn pending_dpc_rejects_rsp_commit_without_mutating_rsp_state() {
        with_host(|host| *host = HostState::default());
        let mut live_rdram = vec![0x5a; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let (task_addr, task_generation) = prepare_verified_audio_rdram(&mut live_rdram);
        let pending = with_host(|host| {
            host.device_fabric
                .request_dpc_submission(fn64_runtime::DpcSubmissionSource::Rdram, 0x100, 0x108)
                .unwrap()
        })
        .expect("unfrozen DPC submission must publish");
        let before_rdram = live_rdram.clone();
        let (before_memory, before_registers) = with_host(|host| {
            (
                host.device_fabric.rsp_memory().snapshot(),
                host.device_fabric.rsp_execution_state(),
            )
        });
        let mut replacement = fn64_runtime::RspMemory::new();
        replacement
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &[0xde, 0xad, 0xbe, 0xef],
            )
            .unwrap();
        let mut replacement_registers = before_registers;
        replacement_registers.pc = 0x080;
        let mut machine_storage = vec![0; 0x1000];
        let mut machine = fn64_audio::rsp::runtime::RspMachine::new(&mut machine_storage);
        machine.set_sp_status_raw(replacement_registers.sp_status);
        let machine_state = machine.snapshot_state();
        let patches = fn64_audio::hle_outcome::CanonicalRdramPatches::new(vec![
            fn64_audio::hle_outcome::RdramPatch::new(1, vec![1, 2, 3]).unwrap(),
        ])
        .unwrap();

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            unsafe {
                commit_verified_audio_effects(
                    live_rdram.as_mut_ptr(),
                    task_addr,
                    task_generation,
                    patches,
                    replacement.snapshot(),
                    machine_state,
                    replacement_registers.pc,
                    Vec::new(),
                )
            };
        }));

        assert!(rejected.is_err());
        assert_eq!(live_rdram, before_rdram);
        with_host(|host| {
            assert_eq!(host.device_fabric.rsp_memory().snapshot(), before_memory);
            assert_eq!(host.device_fabric.rsp_execution_state(), before_registers);
            host.device_fabric
                .cancel_dpc_submission(pending.token)
                .unwrap();
        });
    }


    #[test]
    fn deferred_audio_dpc_words_survive_source_mutation() {
        let captured = vec![0xe900_0000, 0, 0x1122_3344, 0x5566_7788];
        let deferred = fn64_audio::hle_outcome::DeferredDpcSubmission::from_rdram_words(
            0x100,
            0x110,
            captured.clone(),
        )
        .unwrap();
        let mut later_source = captured;

        later_source.fill(0xa5a5_a5a5);

        assert_eq!(
            deferred.command_words(),
            vec![0xe900_0000, 0, 0x1122_3344, 0x5566_7788]
        );
        assert_ne!(deferred.command_words(), later_source);
    }


    #[test]
    fn deferred_audio_dpc_conversion_preserves_owned_identity() {
        let deferred = fn64_audio::hle_outcome::DeferredDpcSubmission::from_rdram_words(
            0x100,
            0x108,
            vec![0xe900_0000, 0],
        )
        .unwrap();
        let expected_words = deferred.command_words();

        let batch = deferred_audio_dpc_batch(vec![deferred]).unwrap();

        assert_eq!(batch.submissions().len(), 1);
        assert_eq!(
            batch.submissions()[0].source(),
            fn64_render::RawDpcSource::Rdram
        );
        assert_eq!(batch.submissions()[0].command_words(), expected_words);
    }


    #[test]
    fn verified_audio_empty_dpc_batch_needs_no_renderer() {
        with_host(|host| *host = HostState::default());
        RENDER_BACKEND.with(|cell| cell.replace(None));
        let mut live = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let (task_addr, task_generation) = prepare_verified_audio_rdram(&mut live);
        let machine = verified_audio_test_machine();
        let expected = machine.architectural_state().clone();

        let status = unsafe {
            commit_verified_audio_effects(
                live.as_mut_ptr(),
                task_addr,
                task_generation,
                empty_verified_audio_patches(),
                fn64_runtime::RspMemory::new().snapshot(),
                machine,
                0,
                Vec::new(),
            )
        };

        assert_eq!(status, fn64_render::DpFullSyncStatus::NotReached);
        assert_eq!(
            crate::host_evidence_snapshot().rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::Exact(expected)
        );
    }


    #[test]
    fn verified_audio_diagnostic_dpc_rejects_before_live_mutation() {
        use fn64_render_reference::ReferenceBackend;

        with_host(|host| *host = HostState::default());
        let mut live = vec![0x5a; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let (task_addr, task_generation) = prepare_verified_audio_rdram(&mut live);
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
        set_render_backend(Box::new(backend), live.len());
        let before_rdram = live.clone();
        let before_host = crate::host_evidence_snapshot();

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            commit_verified_audio_effects(
                live.as_mut_ptr(),
                task_addr,
                task_generation,
                empty_verified_audio_patches(),
                fn64_runtime::RspMemory::new().snapshot(),
                verified_audio_test_machine(),
                0,
                vec![full_sync_deferred_submission()],
            )
        }));

        assert!(rejected.is_err());
        assert_eq!(live, before_rdram);
        assert_eq!(crate::host_evidence_snapshot(), before_host);
    }


    #[cfg(feature = "recomp-rs")]
    #[test]
    fn verified_audio_identical_patch_rejects_planned_executable_overlap_without_mutation() {
        with_host(|host| *host = HostState::default());
        let mut live = vec![0x5a; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let (task_addr, task_generation) = prepare_verified_audio_rdram(&mut live);
        let executable_start = 0x120;
        let _preflight = crate::recompiled::scoped_test_executable_write_preflight_state(
            vec![(executable_start, executable_start + 0x40)],
            Vec::new(),
        );
        let patches = fn64_audio::hle_outcome::CanonicalRdramPatches::new(vec![
            fn64_audio::hle_outcome::RdramPatch::new(executable_start, vec![0x5a; 4]).unwrap(),
        ])
        .unwrap();
        let mut replacement_memory = fn64_runtime::RspMemory::new();
        replacement_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0x20),
                &[0xde, 0xad, 0xbe, 0xef],
            )
            .unwrap();
        let before_rdram = live.clone();
        let before_host = crate::host_evidence_snapshot();
        let (before_device, before_rsp_memory) = with_host(|host| {
            (
                host.device_fabric.rsp_execution_state(),
                host.device_fabric.rsp_memory().snapshot(),
            )
        });

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            commit_verified_audio_effects(
                live.as_mut_ptr(),
                task_addr,
                task_generation,
                patches,
                replacement_memory.snapshot(),
                verified_audio_test_machine(),
                0x80,
                Vec::new(),
            )
        }));

        assert!(rejected.is_err());
        assert_eq!(live, before_rdram);
        assert_eq!(crate::host_evidence_snapshot(), before_host);
        with_host(|host| {
            assert_eq!(host.device_fabric.rsp_execution_state(), before_device);
            assert_eq!(
                host.device_fabric.rsp_memory().snapshot(),
                before_rsp_memory
            );
        });
    }


    #[cfg(feature = "recomp-rs")]
    #[test]
    fn verified_audio_pending_executable_overlap_rejects_empty_publication_without_mutation() {
        with_host(|host| *host = HostState::default());
        let mut live = vec![0x5a; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let (task_addr, task_generation) = prepare_verified_audio_rdram(&mut live);
        let _preflight = crate::recompiled::scoped_test_executable_write_preflight_state(
            vec![(0x100, 0x180)],
            vec![(0x120, 4)],
        );
        let mut replacement_memory = fn64_runtime::RspMemory::new();
        replacement_memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0x20),
                &[0xde, 0xad, 0xbe, 0xef],
            )
            .unwrap();
        let before_rdram = live.clone();
        let before_host = crate::host_evidence_snapshot();
        let (before_device, before_rsp_memory) = with_host(|host| {
            (
                host.device_fabric.rsp_execution_state(),
                host.device_fabric.rsp_memory().snapshot(),
            )
        });

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            commit_verified_audio_effects(
                live.as_mut_ptr(),
                task_addr,
                task_generation,
                empty_verified_audio_patches(),
                replacement_memory.snapshot(),
                verified_audio_test_machine(),
                0x80,
                Vec::new(),
            )
        }));

        assert!(rejected.is_err());
        assert_eq!(live, before_rdram);
        assert_eq!(crate::host_evidence_snapshot(), before_host);
        with_host(|host| {
            assert_eq!(host.device_fabric.rsp_execution_state(), before_device);
            assert_eq!(
                host.device_fabric.rsp_memory().snapshot(),
                before_rsp_memory
            );
        });
    }


    #[test]
    fn verified_audio_wrong_task_owner_rejects_before_live_mutation() {
        with_host(|host| *host = HostState::default());
        let mut live = vec![0x5a; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let (_owner, task_generation) = prepare_verified_audio_rdram(&mut live);
        let before_rdram = live.clone();
        let before_host = crate::host_evidence_snapshot();

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            commit_verified_audio_effects(
                live.as_mut_ptr(),
                RdramAddr::from_offset(VERIFIED_AUDIO_TASK_OFFSET + 8),
                task_generation,
                empty_verified_audio_patches(),
                fn64_runtime::RspMemory::new().snapshot(),
                verified_audio_test_machine(),
                0,
                Vec::new(),
            )
        }));

        assert!(rejected.is_err());
        assert_eq!(live, before_rdram);
        assert_eq!(crate::host_evidence_snapshot(), before_host);
    }


    #[test]
    fn verified_audio_same_address_reuse_rejects_stale_generation() {
        with_host(|host| *host = HostState::default());
        let mut live = vec![0x5a; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        let (task_addr, stale_generation) = prepare_verified_audio_rdram(&mut live);
        retain_loaded_rsp_task(PendingLoadedRspTask {
            task_addr,
            header: OsTaskHeader::default(),
            resumed_data_identity: None,
        });
        let replacement = take_loaded_rsp_task(task_addr);
        let replacement_generation = replacement.admission_generation;
        retain_started_rsp_task_lineage(replacement, None);
        assert_ne!(replacement_generation.get(), stale_generation.get());
        let before_rdram = live.clone();
        let before_host = crate::host_evidence_snapshot();

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            commit_verified_audio_effects(
                live.as_mut_ptr(),
                task_addr,
                stale_generation,
                empty_verified_audio_patches(),
                fn64_runtime::RspMemory::new().snapshot(),
                verified_audio_test_machine(),
                0,
                Vec::new(),
            )
        }));

        assert!(rejected.is_err());
        assert_eq!(live, before_rdram);
        assert_eq!(crate::host_evidence_snapshot(), before_host);
    }


    #[test]
    fn render_task_maps_kseg0_display_list_pointer_to_a_physical_offset() {
        // Regression: WM2000 submits its display list at a KSEG0 virtual
        // address (0x8038ce30). Before masking, this reached rt64 ingress raw
        // and tripped "display-list address 0x8038ce30 is not a physical RDRAM
        // offset", panicking the shell on its first gfx task.
        let header = OsTaskHeader {
            task_type: fn64_runtime::M_GFXTASK,
            data_ptr: 0x8038_ce30,
            ..Default::default()
        };
        assert_eq!(render_task(&header).data_ptr, 0x0038_ce30);
        // An already-physical pointer passes through unchanged.
        let physical = OsTaskHeader {
            data_ptr: 0x0038_ce30,
            ..Default::default()
        };
        assert_eq!(render_task(&physical).data_ptr, 0x0038_ce30);
    }


    #[test]
    fn generated_c_kseg1_same_value_halfword_keeps_visible_bytes_and_renderer_sidecar_coherent() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct WriteCaptureBackend {
            write: Rc<Cell<Option<fn64_render::NonRdpWrite16>>>,
            hidden_bits: Rc<Cell<Option<u8>>>,
        }

        impl RenderBackend for WriteCaptureBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            fn observe_non_rdp_write16(
                &mut self,
                write: fn64_render::NonRdpWrite16,
            ) -> fn64_render::NonRdpWrite16Disposition {
                self.write.set(Some(write));
                self.hidden_bits
                    .set(Some(if write.value() & 1 == 0 { 0 } else { 3 }));
                fn64_render::NonRdpWrite16Disposition::AppliedHiddenSidecar
            }

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                Ok(FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        RENDER_BACKEND.with(|cell| cell.replace(None));
        assert_eq!(observe_non_rdp_write16(0x40, 0x1235), None);

        let captured = Rc::new(Cell::new(None));
        let hidden_bits = Rc::new(Cell::new(None));
        set_render_backend(
            Box::new(WriteCaptureBackend {
                write: captured.clone(),
                hidden_bits: hidden_bits.clone(),
            }),
            0x100,
        );
        let mut visible = vec![0u8; 0x100];
        fn64_runtime::RdramViewMut::from_storage(&mut visible)
            .write_u16(fn64_runtime::RdramAddr::from_offset(0x40), 0x1235);
        crate::fn64_c_rdram_write(0xffff_ffff_a000_0040, 2, 0x1235);
        let write = captured
            .get()
            .expect("generated-C SH event was not delivered");
        assert_eq!(write.logical_offset().offset(), 0x40);
        assert_eq!(write.value(), 0x1235);
        assert_eq!(
            fn64_runtime::RdramView::from_storage(&visible)
                .read_u16(fn64_runtime::RdramAddr::from_offset(0x40)),
            0x1235
        );
        assert_eq!(hidden_bits.get(), Some(3));

        // A second identical assignment is still a distinct architectural
        // write and must be delivered rather than suppressed by equality.
        captured.set(None);
        crate::fn64_c_rdram_write(0xffff_ffff_a000_0040, 2, 0x1235);
        assert_eq!(captured.get(), Some(write));
        assert_eq!(hidden_bits.get(), Some(3));
    }


    #[test]
    fn generated_c_rdram_callback_rejects_mapped_aliases() {
        assert_eq!(
            crate::generated_c_rdram_physical_offset(0xffff_ffff_8000_0040),
            Some(0x40)
        );
        assert_eq!(
            crate::generated_c_rdram_physical_offset(0xffff_ffff_a000_0040),
            Some(0x40)
        );
        assert_eq!(
            crate::generated_c_rdram_physical_offset(0x0000_0000_8000_0040),
            Some(0x40)
        );
        assert_eq!(
            crate::generated_c_rdram_physical_offset(0x0000_0000_a000_0040),
            Some(0x40)
        );
        assert_eq!(crate::generated_c_rdram_physical_offset(0x40), None);
        assert_eq!(
            crate::generated_c_rdram_physical_offset(0xffff_ffff_c000_0040),
            None
        );
        assert_eq!(
            crate::generated_c_rdram_physical_offset(0x0000_0001_8000_0040),
            None
        );
    }


    #[test]
    fn graphics_backend_receives_the_device_fabrics_persistent_rsp_memory() {
        struct RspMemoryBackend;

        impl RenderBackend for RspMemoryBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                rsp_memory
                    .write_bytes(fn64_runtime::RspMemAddr::from_register(0x120), b"rsp-live")
                    .unwrap();
                Ok(FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        let mut rdram = vec![0u8; 0x1000];
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(Box::new(RspMemoryBackend), rdram.len());
        let header = OsTaskHeader {
            task_type: fn64_runtime::M_GFXTASK,
            ..Default::default()
        };
        let status = unsafe { dispatch_gfx_task(rdram.as_mut_ptr(), &header) };
        assert_eq!(status.status, FrameStatus::Complete);
        assert_eq!(
            status.dp_full_sync,
            fn64_render::DpFullSyncStatus::Unidentified
        );
        with_host(|host| {
            assert_eq!(
                host.device_fabric
                    .rsp_memory()
                    .read_bytes(fn64_runtime::RspMemAddr::from_register(0x120), 8)
                    .unwrap(),
                b"rsp-live"
            );
        });
    }


    #[test]
    fn every_renderer_entry_traps_when_no_backend_is_registered() {
        RENDER_BACKEND.with(|cell| cell.replace(None));
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_render_backend::<()>("renderer_gate_test", |_| Ok(()));
        }))
        .expect_err("missing renderer must panic");
        set_render_backend(Box::new(StatusRenderBackend(FrameStatus::Complete)), 0);
        assert!(panic_message(panic.as_ref())
            .contains("renderer_gate_test: no render backend registered"));
    }


    #[test]
    fn unsupported_backend_ucode_records_typed_event_before_loud_failure() {
        set_render_backend(Box::new(UnsupportedUcodeBackend), 0);
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let mut rdram = [];
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        let task = fn64_render::OsTask {
            ucode: 0x0012_3450,
            ..Default::default()
        };

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_render_backend::<FrameStatus>("unsupported_ucode_test", |backend| {
                backend.process_task(&mut rdram, &mut rsp_memory, &task, 0)
            });
        }))
        .expect_err("unsupported backend ucode must remain a loud failure");

        assert!(
            panic_message(panic.as_ref()).contains("unsupported ucode at rdram offset 0x00123450")
        );
        assert_eq!(
            last_render_error().as_deref(),
            Some("unsupported ucode at rdram offset 0x00123450")
        );
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].subsystem,
            fn64_runtime::UnsupportedSubsystem::Render
        );
        assert_eq!(events[0].operation, "render.backend.unsupported-ucode");
        assert_eq!(
            events[0].context,
            "unsupported_ucode_test: backend rejected unlisted microcode at RDRAM offset 0x00123450"
        );
        assert_eq!(events[0].guest_cycle, Some(fn64_runtime::Cycles::ZERO));
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::LoudTrap
        );
        fn64_runtime::complete_unsupported_observation(fn64_runtime::Cycles::ZERO, &"0".repeat(64));
        RENDER_BACKEND.with(|cell| cell.replace(None));
    }


    #[test]
    fn release_capture_crosses_the_owned_renderer_seam_without_downcasting() {
        struct CaptureBackend;

        impl RenderBackend for CaptureBackend {
            fn release_environment(&self) -> fn64_render::RenderBackendEvidence {
                fn64_render::RenderBackendEvidence::Rt64 {
                    tv_type: fn64_runtime::TvType::Pal,
                    backend_identity: "synthetic-release-backend".to_string(),
                    source_authoritative: true,
                    graphics_api: fn64_render::ActiveRenderGraphicsApi::Vulkan,
                    settings_sha256: [0x5a; 32],
                    replacement_packs_active: false,
                }
            }

            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                Ok(FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn release_capture(&mut self) -> Result<fn64_render::RenderReleaseCapture, RenderError> {
                self.release_capture_into(&mut Vec::new())
            }

            fn release_capture_into(
                &mut self,
                reuse: &mut Vec<u8>,
            ) -> Result<fn64_render::RenderReleaseCapture, RenderError> {
                reuse.clear();
                reuse.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
                Ok(fn64_render::RenderReleaseCapture {
                    guest_cycle: 0x1234,
                    backend_identity: "synthetic-release-backend".to_string(),
                    source_authoritative: true,
                    settings_sha256: [0x5a; 32],
                    pixels: fn64_render::ReleaseCapturePixels::try_from_reused(
                        fn64_render::ReleaseCaptureLayout::try_new(
                            fn64_render::ReleaseCaptureLayoutSpec {
                                format: fn64_render::ReleaseCaptureFormat::PostViBgra8Unorm,
                                width: 2,
                                storage_height: 1,
                                visible_height: 1,
                                row_bytes: 8,
                            },
                        )
                        .unwrap(),
                        reuse,
                    )
                    .unwrap(),
                    workload_id: std::num::NonZeroU64::new(5).unwrap(),
                    present_id: 7,
                })
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        set_render_backend(Box::new(CaptureBackend), 0);
        let capture = capture_render_release_frame().unwrap();
        assert_eq!(capture.guest_cycle, 0x1234);
        assert_eq!(capture.backend_identity, "synthetic-release-backend");
        assert!(capture.source_authoritative);
        assert_eq!(capture.workload_id.get(), 5);
        assert_eq!(capture.settings_sha256, [0x5a; 32]);
        assert_eq!(capture.present_id, 7);
        assert_eq!(capture.bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(last_render_error(), None);
        assert_eq!(
            render_environment_evidence_snapshot(),
            RenderEnvironmentEvidenceSnapshot {
                backend: fn64_render::RenderBackendEvidence::Rt64 {
                    tv_type: fn64_runtime::TvType::Pal,
                    backend_identity: "synthetic-release-backend".to_string(),
                    source_authoritative: true,
                    graphics_api: fn64_render::ActiveRenderGraphicsApi::Vulkan,
                    settings_sha256: [0x5a; 32],
                    replacement_packs_active: false,
                },
                execution_policy: GraphicsTaskExecutionPolicy::HleOptimized,
            }
        );
        assert_eq!(
            render_environment_evidence_snapshot().renderer_tv_type(),
            Some(fn64_runtime::TvType::Pal)
        );

        set_render_backend(Box::new(CaptureBackend), 0);
        let mut reuse = Vec::with_capacity(64);
        let allocation = reuse.as_ptr();
        let capture = capture_render_release_frame_into(&mut reuse).unwrap();
        assert_eq!(capture.bytes.as_ptr(), allocation);
        reuse = capture.pixels.into_bytes();
        assert!(reuse.capacity() >= 64);
        assert_eq!(reuse, [1, 2, 3, 4, 5, 6, 7, 8]);
    }


    #[test]
    fn unidentified_renderer_snapshot_cannot_fabricate_tv_authority() {
        set_render_backend(Box::new(StatusRenderBackend(FrameStatus::Complete)), 0);
        let snapshot = render_environment_evidence_snapshot();
        assert_eq!(
            snapshot.backend,
            fn64_render::RenderBackendEvidence::Unidentified
        );
        assert_eq!(snapshot.renderer_tv_type(), None);
    }


    #[test]
    fn rsp_visibility_excludes_host_only_address_windows() {
        assert_eq!(
            rsp_visible_rdram_len(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE + 0x2490_0000),
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE
        );
        assert_eq!(rsp_visible_rdram_len(0x1000), 0x1000);

        let (ranges, snapshot_len) = rsp_dma_storage_layout(
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE + 0x2000,
            std::iter::once(0x80_0000..0x80_1000).collect(),
        );
        assert_eq!(
            ranges,
            vec![
                0..fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
                0x80_0000..0x80_1000
            ]
        );
        assert_eq!(snapshot_len, 0x80_1000);
    }


    #[test]
    fn lle_debug_task_data_preserves_logical_order_at_the_rdram_boundary() {
        let logical = [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87];
        let mut storage = [0u8; 8];
        fn64_runtime::RdramViewMut::from_storage(&mut storage)
            .write_logical_bytes(RdramAddr::from_offset(0), &logical);

        assert_eq!(
            lle_debug_task_data(&storage, 0xab00_0001, 1).as_deref(),
            Some(&logical[1..]),
            "the diagnostic's 0x40-byte minimum must truncate at RDRAM's end in guest byte order"
        );
        assert_eq!(
            lle_debug_task_data(&storage, storage.len() as u32, 1),
            None,
            "a task-data start at the allocation boundary must not create an empty dump"
        );
    }


    #[test]
    fn lle_debug_task_data_loudly_rejects_an_unmapped_native_word_lane() {
        let storage = [0u8; 7];
        let panic = std::panic::catch_unwind(|| lle_debug_task_data(&storage, 4, 1))
            .expect_err("an incomplete final native word must trap instead of supplying zero");
        assert!(panic_message(panic.as_ref())
            .contains("read_u8: logical RDRAM range 0x4..0x5 maps outside 7 storage bytes"));
    }


    #[test]
    fn renderer_entries_expose_exact_physical_rdram_and_its_last_byte() {
        use std::rc::Rc;

        crate::load_rom(Vec::new());

        struct SpanBackend(Rc<RefCell<Vec<(usize, u8)>>>);

        impl RenderBackend for SpanBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                self.0
                    .borrow_mut()
                    .push((rdram.len(), *rdram.last().unwrap()));
                Ok(FrameStatus::Complete)
            }

            fn process_rdp_commands(
                &mut self,
                rdram: &mut [u8],
                _start: u32,
                _end: u32,
                _output_addr: u32,
                _wait_for_completion: bool,
            ) -> Result<FrameStatus, RenderError> {
                self.0
                    .borrow_mut()
                    .push((rdram.len(), *rdram.last().unwrap()));
                Ok(FrameStatus::Complete)
            }

            fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
                fn64_render::DpFullSyncStatus::NotReached
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        let physical_len = fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
        let mut allocation = vec![0u8; physical_len + 0x2000];
        allocation[physical_len - 1] = 0xa5;
        allocation[physical_len] = 0x5a;
        let observations = Rc::new(RefCell::new(Vec::new()));
        set_render_backend(
            Box::new(SpanBackend(Rc::clone(&observations))),
            allocation.len(),
        );

        let header = OsTaskHeader {
            task_type: fn64_runtime::M_GFXTASK,
            ..Default::default()
        };
        unsafe {
            dispatch_gfx_task(allocation.as_mut_ptr(), &header);
            dispatch_raw_rdp(allocation.as_mut_ptr(), 0, 8);
        }

        assert_eq!(observations.borrow().as_slice(), [(physical_len, 0xa5); 2]);
        assert_eq!(allocation[physical_len], 0x5a);
        assert_eq!(
            copy_rsp_rdp_observations()
                .into_iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![RspRdpObservationKind::DramDpcCommitted {
                start: 0,
                end: 8,
                command_sha256: canonical_rdp_words_sha256(&[0, 0]),
            }]
        );
    }


    #[test]
    fn renderer_entry_rejects_a_registration_shorter_than_physical_rdram() {
        let mut allocation = [0u8; 1];
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::Complete)),
            allocation.len(),
        );
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_gfx_task(allocation.as_mut_ptr(), &OsTaskHeader::default())
        }))
        .expect_err("a short renderer registration must trap before constructing a slice");
        assert!(panic_message(panic.as_ref()).contains("does not cover the required 8 MiB"));
    }


    #[test]
    fn completed_renderer_without_dp_full_sync_evidence_traps() {
        let panic = std::panic::catch_unwind(|| {
            rcp_completion_plan(
                fn64_render::DpFullSyncStatus::Unidentified,
                "synthetic completed renderer",
            )
        })
        .expect_err("successful graphics completion must identify FullSync state");
        assert!(panic_message(panic.as_ref()).contains(
            "synthetic completed renderer: renderer completed without identifying DP FullSync state"
        ));
    }


    #[test]
    fn every_renderer_entry_traps_and_records_a_backend_error() {
        set_render_backend(Box::new(StatusRenderBackend(FrameStatus::Complete)), 0);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_render_backend::<()>("renderer_gate_test", |_| {
                Err(RenderError::Backend {
                    backend: "synthetic",
                    reason: "deliberate failure".to_owned(),
                })
            });
        }))
        .expect_err("renderer error must panic");
        assert!(panic_message(panic.as_ref())
            .contains("renderer_gate_test: synthetic backend error: deliberate failure"));
        assert_eq!(
            last_render_error().as_deref(),
            Some("synthetic backend error: deliberate failure")
        );
    }


    #[test]
    fn rejected_raw_dpc_does_not_enter_the_committed_observation_history() {
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::Complete)),
            rdram.len(),
        );

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_raw_rdp(rdram.as_mut_ptr(), 0, 8)
        }));

        assert!(rejected.is_err());
        assert!(copy_rsp_rdp_observations().is_empty());
        let snapshot = with_host(|host| host.device_fabric.snapshot());
        assert_eq!(snapshot.pending_dpc, None);
        assert_eq!(snapshot.dpc_start, 0);
        assert_eq!(snapshot.dpc_end, 0);
        assert_eq!(snapshot.dpc_current, 0);
        assert_eq!(snapshot.dpc_status, 0);
    }

    #[test]
    fn raw_mmio_freeze_holds_renderer_work_until_clear_freeze() {
        const START: usize = 0x100;
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        rdram[START..START + 4].copy_from_slice(&0xe900_0000u32.to_ne_bytes());
        let calls = Rc::new(Cell::new(0));
        set_render_backend(
            Box::new(MutatingRawBackend {
                calls: Rc::clone(&calls),
                outcome: RawMutationOutcome::Complete,
                mutation_offset: 0x400,
            }),
            rdram.len(),
        );
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });

        assert!(crate::pi::write_live_device_mmio(
            0xffff_ffff_a410_000c,
            0x08
        ));
        assert!(crate::pi::write_live_device_mmio(
            0xffff_ffff_a410_0000,
            START as u32
        ));
        assert!(crate::pi::write_live_device_mmio(
            0xffff_ffff_a410_0004,
            (START + 8) as u32
        ));
        assert_eq!(calls.get(), 0, "frozen END escaped to the renderer");
        assert_eq!(
            crate::pi::read_live_device_mmio(0xffff_ffff_a410_0008),
            Some(START as u32)
        );

        assert!(crate::pi::write_live_device_mmio(
            0xffff_ffff_a410_000c,
            0x04
        ));
        assert_eq!(calls.get(), 1, "clear-freeze did not release renderer work");
        assert_eq!(
            crate::pi::read_live_device_mmio(0xffff_ffff_a410_0008),
            Some((START + 8) as u32)
        );
    }

    #[test]
    fn rejected_raw_renderer_mutations_never_reach_live_rdram() {
        const START: usize = 0x100;
        const MUTATION: usize = 0x400;

        for (outcome, expected) in [
            (RawMutationOutcome::Error, "mutate-then-error"),
            (RawMutationOutcome::Panic, "mutating raw backend panic"),
            (
                RawMutationOutcome::Yielded,
                "raw RDP submission cannot yield as an RSP task",
            ),
        ] {
            crate::load_rom(Vec::new());
            let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
            rdram[START..START + 4].copy_from_slice(&0xe900_0000u32.to_ne_bytes());
            rdram[MUTATION] = 0x5a;
            let calls = Rc::new(Cell::new(0));
            set_render_backend(
                Box::new(MutatingRawBackend {
                    calls: Rc::clone(&calls),
                    outcome,
                    mutation_offset: MUTATION,
                }),
                rdram.len(),
            );
            let before_rdram = rdram.clone();
            let before_device = with_host(|host| host.device_fabric.snapshot());

            let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                dispatch_raw_rdp(rdram.as_mut_ptr(), START as u32, (START + 8) as u32)
            }))
            .expect_err("mutating raw renderer rejection must remain loud");

            assert!(
                panic_message(rejected.as_ref()).contains(expected),
                "{outcome:?} produced unexpected panic: {}",
                panic_message(rejected.as_ref())
            );
            assert_eq!(calls.get(), 1);
            assert_eq!(rdram, before_rdram, "{outcome:?} leaked renderer bytes");
            assert_eq!(
                with_host(|host| host.device_fabric.snapshot()),
                before_device,
                "{outcome:?} changed guest-visible DPC state"
            );
            assert!(copy_rsp_rdp_observations().is_empty());
        }
    }


    #[test]
    fn mismatched_raw_full_sync_evidence_rejects_before_rdram_or_dpc_commit() {
        const START: usize = 0x100;
        const MUTATION: usize = 0x400;

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        rdram[MUTATION] = 0x5a;
        let calls = Rc::new(Cell::new(0));
        set_render_backend(
            Box::new(MutatingRawBackend {
                calls: Rc::clone(&calls),
                outcome: RawMutationOutcome::Complete,
                mutation_offset: MUTATION,
            }),
            rdram.len(),
        );
        let before_rdram = rdram.clone();
        let before_device = with_host(|host| host.device_fabric.snapshot());

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_raw_rdp(rdram.as_mut_ptr(), START as u32, (START + 8) as u32)
        }))
        .expect_err("backend FullSync evidence must match the submitted command stream");

        assert!(panic_message(rejected.as_ref()).contains(
            "renderer FullSync evidence disagrees with the submitted raw RDP command stream"
        ));
        assert_eq!(calls.get(), 1);
        assert_eq!(rdram, before_rdram);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot()),
            before_device
        );
        assert!(copy_rsp_rdp_observations().is_empty());
    }


    #[test]
    fn mismatched_captured_full_sync_evidence_rejects_before_rdram_or_dpc_commit() {
        const MUTATION: usize = 0x400;

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        rdram[MUTATION] = 0x5a;
        let dmem = [0u8; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let calls = Rc::new(Cell::new(0));
        set_render_backend(
            Box::new(MutatingRawBackend {
                calls: Rc::clone(&calls),
                outcome: RawMutationOutcome::Complete,
                mutation_offset: MUTATION,
            }),
            rdram.len(),
        );
        let before_rdram = rdram.clone();
        let before_device = with_host(|host| host.device_fabric.snapshot());

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_raw_rdp_xbus(rdram.as_mut_ptr(), &dmem, 0, 8)
        }))
        .expect_err("captured FullSync evidence must match the staged command stream");

        assert!(panic_message(rejected.as_ref()).contains(
            "renderer FullSync evidence disagrees with the submitted raw RDP command stream"
        ));
        assert_eq!(calls.get(), 1);
        assert_eq!(rdram, before_rdram);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot()),
            before_device
        );
        assert!(copy_rsp_rdp_observations().is_empty());
    }


    #[test]
    fn atomic_ack_validation_failure_precedes_xbus_publication_and_rolls_back() {
        const MUTATION: usize = 0x400;

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        rdram[MUTATION] = 0x5a;
        let before_rdram = rdram.clone();
        let before_device = with_host(|host| host.device_fabric.snapshot());
        let calls = Rc::new(Cell::new(0));
        set_render_backend(
            Box::new(MutatingRawBackend {
                calls: Rc::clone(&calls),
                outcome: RawMutationOutcome::Complete,
                mutation_offset: MUTATION,
            }),
            rdram.len(),
        );
        let submission = with_host(|host| {
            host.device_fabric
                .request_dpc_submission(fn64_runtime::DpcSubmissionSource::Dmem, 0, 8)
        })
        .unwrap()
        .expect("unfrozen DPC submission must publish");
        let mut transaction = LiveDpcTransaction::new(submission);
        let fn64_runtime::DpcScheduledPhase::AwaitingAck(request) = transaction
            .acknowledgment
            .as_ref()
            .expect("test transaction owns its atomic acknowledgment")
            .phase()
        else {
            panic!("production atomic transaction did not stop at its sole ack barrier")
        };
        assert_eq!(
            request.transaction,
            fn64_runtime::DpcTransactionId::from_submission(submission)
        );
        assert_eq!(request.quantum, fn64_runtime::DpcQuantumId::new(1));
        assert_eq!(request.start.source(), submission.source);
        assert_eq!(request.start.address(), submission.start);
        assert_eq!(request.end.source(), submission.source);
        assert_eq!(request.end.address(), submission.end);
        transaction
            .acknowledgment
            .as_mut()
            .expect("test transaction owns its atomic acknowledgment")
            .poison();

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_captured_raw_rdp(
                rdram.as_mut_ptr(),
                &[0xe900_0000, 0],
                0,
                8,
                true,
                true,
                &mut transaction,
            )
        }))
        .expect_err("poisoned atomic acknowledgment must remain loud");
        assert!(panic_message(rejected.as_ref())
            .contains("lost its acknowledgment owner before validation"));
        assert_eq!(
            calls.get(),
            1,
            "validation remains after backend acceptance"
        );
        assert_eq!(rdram, before_rdram, "ack failure published the XBUS shadow");
        assert!(copy_rsp_rdp_observations().is_empty());

        drop(transaction);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot()),
            before_device,
            "ack failure did not restore the pre-admission DPC state"
        );
    }
