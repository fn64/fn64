use super::*;
use fn64_cpu_runtime::CodeSpan;

    #[test]
    fn translated_cpu_unsupported_gap_records_the_typed_release_event() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        record_recompiled_unsupported("unsupported COP0 register 7");

        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].subsystem,
            fn64_runtime::UnsupportedSubsystem::Recompiler
        );
        assert_eq!(
            events[0].operation,
            concat!("recompiler.cpu.", "unsupported-instruction")
        );
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::LoudTrap
        );
        assert!(events[0].guest_cycle.is_some());
    }


    #[test]
    fn function_lane_evidence_requires_identity_and_excludes_callable_pointers() {
        with_host(|host| *host = crate::HostState::default());
        set_entry_lookup(evidence_lookup, 0x100);
        let missing = std::panic::catch_unwind(recompiled_program_evidence_snapshot)
            .expect_err("unidentified function lane must fail evidence capture");
        let message = missing
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| missing.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(message.contains("stable host-provided artifact identity"));

        let identity = ProgramArtifactIdentity::new([0xA5; 32]);
        set_entry_lookup_with_artifact_identity(evidence_lookup, 0x100, identity);
        let first = recompiled_program_evidence_snapshot().unwrap();
        set_entry_lookup_with_artifact_identity(alternate_evidence_lookup, 0x100, identity);
        assert_eq!(first, recompiled_program_evidence_snapshot().unwrap());

        set_entry_lookup_with_artifact_identity(
            evidence_lookup,
            0x100,
            ProgramArtifactIdentity::new([0x5A; 32]),
        );
        assert_ne!(first, recompiled_program_evidence_snapshot().unwrap());
    }


    #[test]
    fn function_destination_history_binds_artifact_function_cycle_and_schema() {
        with_host(|host| *host = crate::HostState::default());
        let identity = ProgramArtifactIdentity::new([0xC3; 32]);
        set_entry_lookup_with_execution_observation(
            evidence_lookup,
            0x100,
            identity,
            fn64_cpu_runtime::FUNCTION_ENTRY_OBSERVATION_SCHEMA,
        );
        with_executor(|executor| executor.advance_time(37));
        fn64_cpu_runtime::notify_function_entry(TranslatedFunctionIdentity::new(
            0x8000_1000,
            "entry",
        ));
        with_executor(|executor| executor.advance_time(41));
        fn64_cpu_runtime::notify_function_entry(TranslatedFunctionIdentity::new(
            0x8000_2000,
            "callee",
        ));

        assert_eq!(
            copy_function_execution_destinations(),
            vec![
                FunctionExecutionDestinationObservation {
                    at: fn64_runtime::Cycles::new(37),
                    artifact_identity: identity,
                    function: TranslatedFunctionIdentity::new(0x8000_1000, "entry"),
                },
                FunctionExecutionDestinationObservation {
                    at: fn64_runtime::Cycles::new(41),
                    artifact_identity: identity,
                    function: TranslatedFunctionIdentity::new(0x8000_2000, "callee"),
                },
            ]
        );

        set_entry_lookup_with_artifact_identity(evidence_lookup, 0x100, identity);
        let stale = std::panic::catch_unwind(copy_function_execution_destinations)
            .expect_err("identity-only function install must not claim a complete history");
        let message = stale
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| stale.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(message.contains("entry-observation schema"));
    }

    /// The default is complete history, and it must stay complete: the block
    /// lane's equivalent knob defaults to `None` for the same reason
    /// (certification evidence needs every entry). This pins the default so a
    /// bound cannot be introduced silently as a "fix".
    #[test]
    fn function_destination_history_is_complete_by_default() {
        with_host(|host| *host = crate::HostState::default());
        let identity = ProgramArtifactIdentity::new([0xC4; 32]);
        set_entry_lookup_with_execution_observation(
            evidence_lookup,
            0x100,
            identity,
            fn64_cpu_runtime::FUNCTION_ENTRY_OBSERVATION_SCHEMA,
        );

        for index in 0..64u32 {
            fn64_cpu_runtime::notify_function_entry(TranslatedFunctionIdentity::new(
                0x8000_0000 + index,
                "entry",
            ));
        }

        assert_eq!(copy_function_execution_destinations().len(), 64);
    }

    /// A bound retains the MOST RECENT entries and drops the oldest, matching
    /// `set_block_host_boundary_history_limit`. Asserting the retained vrams
    /// (not just the length) is what separates "bounded" from "stopped
    /// recording after N".
    #[test]
    fn function_destination_history_limit_retains_the_newest_entries() {
        with_host(|host| *host = crate::HostState::default());
        let identity = ProgramArtifactIdentity::new([0xC5; 32]);
        set_entry_lookup_with_execution_observation(
            evidence_lookup,
            0x100,
            identity,
            fn64_cpu_runtime::FUNCTION_ENTRY_OBSERVATION_SCHEMA,
        );
        set_function_execution_destination_history_limit(std::num::NonZeroUsize::new(4));

        for index in 0..64u32 {
            fn64_cpu_runtime::notify_function_entry(TranslatedFunctionIdentity::new(
                0x8000_0000 + index,
                "entry",
            ));
        }

        let retained = copy_function_execution_destinations();
        assert_eq!(retained.len(), 4);
        assert_eq!(
            retained
                .iter()
                .map(|observation| observation.function.vram)
                .collect::<Vec<_>>(),
            vec![0x8000_003C, 0x8000_003D, 0x8000_003E, 0x8000_003F],
        );
        set_function_execution_destination_history_limit(None);
    }

    /// Shrinking the bound trims what is already retained, rather than only
    /// applying to entries recorded afterwards.
    #[test]
    fn function_destination_history_limit_trims_existing_entries() {
        with_host(|host| *host = crate::HostState::default());
        let identity = ProgramArtifactIdentity::new([0xC6; 32]);
        set_entry_lookup_with_execution_observation(
            evidence_lookup,
            0x100,
            identity,
            fn64_cpu_runtime::FUNCTION_ENTRY_OBSERVATION_SCHEMA,
        );
        for index in 0..16u32 {
            fn64_cpu_runtime::notify_function_entry(TranslatedFunctionIdentity::new(
                0x8000_0000 + index,
                "entry",
            ));
        }
        assert_eq!(copy_function_execution_destinations().len(), 16);

        set_function_execution_destination_history_limit(std::num::NonZeroUsize::new(3));

        assert_eq!(copy_function_execution_destinations().len(), 3);
        set_function_execution_destination_history_limit(None);
    }

    /// Suppression records nothing at all and clears what was retained. The
    /// paired re-enable proves the switch is not one-way.
    #[test]
    fn function_destination_history_can_be_suppressed_and_re_enabled() {
        with_host(|host| *host = crate::HostState::default());
        let identity = ProgramArtifactIdentity::new([0xC7; 32]);
        set_entry_lookup_with_execution_observation(
            evidence_lookup,
            0x100,
            identity,
            fn64_cpu_runtime::FUNCTION_ENTRY_OBSERVATION_SCHEMA,
        );
        fn64_cpu_runtime::notify_function_entry(TranslatedFunctionIdentity::new(0x8000_1000, "a"));

        set_function_execution_destination_history_enabled(false);
        assert!(copy_function_execution_destinations().is_empty());
        fn64_cpu_runtime::notify_function_entry(TranslatedFunctionIdentity::new(0x8000_2000, "b"));
        assert!(copy_function_execution_destinations().is_empty());

        set_function_execution_destination_history_enabled(true);
        fn64_cpu_runtime::notify_function_entry(TranslatedFunctionIdentity::new(0x8000_3000, "c"));
        assert_eq!(copy_function_execution_destinations().len(), 1);
    }


    #[test]
    fn block_lane_evidence_sorts_regions_and_excludes_builder_pointers() {
        with_host(|host| *host = crate::HostState::default());
        install_evidence_block_lane(8, false, false);
        PENDING_EXECUTABLE_WRITES
            .with(|pending| *pending.borrow_mut() = vec![(0x42, 2), (0x20, 2), (0x21, 3)]);
        let forward = recompiled_program_evidence_snapshot().unwrap();

        install_evidence_block_lane(8, true, true);
        PENDING_EXECUTABLE_WRITES
            .with(|pending| *pending.borrow_mut() = vec![(0x21, 3), (0x42, 2), (0x20, 2)]);
        let reverse = recompiled_program_evidence_snapshot().unwrap();
        assert_eq!(forward, reverse);

        let RecompiledProgramEvidenceSnapshot::Block {
            instruction_budget,
            executable_regions,
            pending_executable_writes,
            ..
        } = forward
        else {
            panic!("block install captured as function lane")
        };
        assert_eq!(instruction_budget, 8);
        assert_eq!(
            executable_regions
                .iter()
                .map(|region| region.physical_start)
                .collect::<Vec<_>>(),
            vec![0x20, 0x40]
        );
        assert_eq!(
            pending_executable_writes,
            vec![
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x20,
                    physical_end: 0x24,
                },
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x42,
                    physical_end: 0x44,
                },
            ]
        );
    }


    #[test]
    fn block_destination_copy_api_reads_the_live_program_history() {
        with_host(|host| *host = crate::HostState::default());
        install_evidence_block_lane(8, false, false);
        assert!(copy_block_execution_destinations().is_empty());
        let live = with_host(|host| {
            host.recompiled_program
                .clone()
                .expect("evidence fixture installs a live block program")
        });
        let entry = ExecutionKey::new(BankId::new(0xE100), GuestPc::new(0x8000_5000));
        let mut bytes = [0u8; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RsContext::new();
        let run = live.program.borrow().run(
            entry,
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert_eq!(run.instructions, 0);
        assert!(matches!(run.exit, BlockExit::Fault(_)));
        assert_eq!(
            copy_block_execution_destinations(),
            vec![ExecutionDestinationObservation {
                destination: entry,
                runner_artifact_identity: Some(ProgramArtifactIdentity::new([0xE5; 32])),
                instructions: 0,
            }]
        );
    }


    #[test]
    fn block_lane_evidence_binds_budget_region_generation_and_pending_writes() {
        with_host(|host| *host = crate::HostState::default());
        install_evidence_block_lane(8, false, false);
        let baseline = recompiled_program_evidence_snapshot().unwrap();

        install_evidence_block_lane(12, false, false);
        let changed_budget = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_budget);

        install_evidence_block_lane(8, false, false);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        live.executable_regions.borrow_mut()[0].next_generation = 2;
        let changed_generation = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_generation);

        install_evidence_block_lane(8, false, false);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        {
            let mut regions = live.executable_regions.borrow_mut();
            regions[0].physical_start += 4;
            regions[0].physical_end += 4;
        }
        let changed_region_geometry = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_region_geometry);

        install_evidence_block_lane(8, false, false);
        with_host(|host| {
            host.recompiled_program
                .as_mut()
                .unwrap()
                .dispatch_artifact_identity = Some(ProgramArtifactIdentity::new([0xD2; 32]));
        });
        let changed_dispatch_artifact = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_dispatch_artifact);

        install_evidence_block_lane(8, false, false);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        live.executable_regions.borrow_mut()[0].builder_artifact_identity =
            Some(ProgramArtifactIdentity::new([0xB2; 32]));
        let changed_builder_artifact = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_builder_artifact);

        install_evidence_block_lane(8, false, false);
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().push((0x30, 4)));
        let changed_pending = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_pending);
    }


    #[test]
    #[should_panic(expected = "pending executable write has zero length")]
    fn block_lane_evidence_never_omits_malformed_pending_write() {
        with_host(|host| *host = crate::HostState::default());
        install_evidence_block_lane(8, false, false);
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().push((0x30, 0)));
        let _ = recompiled_program_evidence_snapshot();
    }


    #[test]
    #[should_panic(expected = "stable host-provided dispatch artifact identity")]
    fn block_lane_evidence_rejects_unidentified_dispatch_artifact() {
        with_host(|host| *host = crate::HostState::default());
        install_evidence_block_lane(8, false, false);
        with_host(|host| {
            host.recompiled_program
                .as_mut()
                .unwrap()
                .dispatch_artifact_identity = None;
        });
        let _ = recompiled_program_evidence_snapshot();
    }


    #[test]
    #[should_panic(expected = "stable host-provided builder artifact identity")]
    fn block_lane_evidence_rejects_unidentified_builder_artifact() {
        with_host(|host| *host = crate::HostState::default());
        install_evidence_block_lane(8, false, false);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        live.executable_regions.borrow_mut()[0].builder_artifact_identity = None;
        let _ = recompiled_program_evidence_snapshot();
    }


    #[test]
    fn verified_host_write_preflight_rejects_only_executable_overlap() {
        let _state = scoped_test_executable_write_preflight_state(vec![(0x100, 0x180)], Vec::new());

        assert_eq!(
            preflight_non_executable_host_writes(&[(0x80, 0x100)]),
            Ok(())
        );
        let overlap = preflight_non_executable_host_writes(&[(0x17f, 0x181)]).unwrap_err();
        assert!(overlap.contains("overlaps live executable region"));
        assert!(overlap.contains("transactional executable publication is unavailable"));

        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().push((0x120, 4)));
        let pending = preflight_non_executable_host_writes(&[]).unwrap_err();
        assert!(pending.contains("pending host write"));
    }


    #[test]
    fn executable_write_preflight_test_scope_restores_on_unwind() {
        let _outer =
            scoped_test_executable_write_preflight_state(vec![(0x20, 0x40)], vec![(0x24, 4)]);

        let panic = std::panic::catch_unwind(|| {
            let _inner = scoped_test_executable_write_preflight_state(
                vec![(0x100, 0x180)],
                vec![(0x120, 8)],
            );
            assert_eq!(
                EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow().clone()),
                vec![(0x100, 0x180)]
            );
            assert_eq!(
                PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
                vec![(0x120, 8)]
            );
            panic!("expected test-scope unwind");
        });

        assert!(panic.is_err());
        assert_eq!(
            EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow().clone()),
            vec![(0x20, 0x40)]
        );
        assert_eq!(
            PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
            vec![(0x24, 4)]
        );
    }


    #[test]
    fn typed_halfword_write_multiplexes_invalidation_and_renderer_once() {
        use std::{cell::Cell, rc::Rc};

        struct CountBackend(Rc<Cell<u32>>);

        impl fn64_render::RenderBackend for CountBackend {
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
                self.0.set(self.0.get() + 1);
                fn64_render::NonRdpWrite16Disposition::AppliedHiddenSidecar
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

            fn resize(&mut self, _width: u32, _height: u32) {}

            fn supported_ucodes(&self) -> &[fn64_render::UcodeId] {
                &[]
            }
        }

        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let renderer_calls = Rc::new(Cell::new(0));
        crate::set_render_backend(Box::new(CountBackend(renderer_calls.clone())), 0x100);
        let previous =
            fn64_cpu_runtime::set_write_observer(Some(record_executable_and_renderer_write));
        let mut bytes = [0u8; 0x100];
        Rdram::new(&mut bytes).store_h(0xffff_ffff_a000_0040, 0x1235);
        fn64_cpu_runtime::set_write_observer(previous);

        assert_eq!(
            PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
            vec![(0x40, 2)]
        );
        assert_eq!(renderer_calls.get(), 1);
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    }


    #[test]
    fn live_block_program_owns_thread_dispatch_and_charges_instruction_time() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let mut bytes = vec![0u8; 0x100];
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(LIVE_ENTRY, GuestPc::new(LIVE_NEXT.get() + 4));
        LIVE_ACTIVE_BANK.with(|active| active.set(LIVE_BANK));
        region
            .install(
                &mut program,
                CodeBank::new(LIVE_BANK, LIVE_ENTRY, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new(LIVE_BANK, live_test_runner),
            )
            .unwrap();
        let thread_id = 0xB10C;
        let previous_host_lookup = fn64_cpu_runtime::set_host_lookup(Some(live_host_lookup));

        // SAFETY: `bytes` remains live until the installed thread has
        // returned and the executor marks it dead below.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(LIVE_BANK, LIVE_ENTRY),
                test_boot_context(LIVE_ENTRY),
                live_entry_lookup,
                live_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 3);
        assert!(!crate::is_thread_dead(thread_id));
        LIVE_ACTIVE_BANK.with(|active| active.set(LIVE_SECOND_BANK));
        assert_eq!(
            install_live_block_generation(
                &mut region,
                CodeBank::new(LIVE_SECOND_BANK, LIVE_ENTRY, vec![1, 1]).unwrap(),
                GeneratedBankRunner::new(LIVE_SECOND_BANK, live_test_runner),
            )
            .unwrap(),
            Some(LIVE_BANK)
        );
        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 5);
        assert!(!crate::is_thread_dead(thread_id));
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000), 0x1234);
        fn64_cpu_runtime::set_host_lookup(previous_host_lookup);
    }


    #[test]
    fn canonical_catalog_boot_owns_dispatch_host_lookup_and_evidence() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let mut bytes = vec![0u8; 0x1008];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(LIVE_BANK, LIVE_ENTRY, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    LIVE_BANK,
                    live_test_runner,
                    ProgramArtifactIdentity::new([0x71; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(LIVE_BANK, LIVE_ENTRY),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(LIVE_HOST.get(), live_host)]).unwrap(),
            ProgramArtifactIdentity::new([0x72; 32]),
        );
        let expected_evidence = install.evidence().clone();
        let previous_host_lookup =
            fn64_cpu_runtime::set_host_lookup(Some(forbidden_catalog_legacy_lookup));
        let thread_id = 0xca70;

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(LIVE_ENTRY),
                thread_id,
                10,
            );
        }

        assert_eq!(
            catalog_resolver_install_evidence_snapshot(),
            Some(expected_evidence)
        );
        assert!(copy_canonical_thread_publications_v1().is_empty());
        assert!(fn64_cpu_runtime::resolve_host_function(LIVE_HOST.get()).is_none());
        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 3);
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(first)] = publications.as_slice() else {
            panic!("expected one exact first checkpoint publication: {publications:?}");
        };
        assert_eq!(first.thread, thread_id);
        assert_eq!(first.charged_instructions, 3);
        assert_eq!(first.canonical_charged_instructions_at_publication, 3);
        assert_eq!(
            first.pending_exit,
            BlockExit::HostCall {
                vram: LIVE_HOST,
                resume: ExecutionKey::new(LIVE_BANK, LIVE_NEXT),
            }
        );
        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 5);
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(second)] = publications.as_slice() else {
            panic!("expected one exact second checkpoint publication: {publications:?}");
        };
        assert_eq!(second.thread, thread_id);
        assert_eq!(second.charged_instructions, 2);
        assert_eq!(second.canonical_charged_instructions_at_publication, 5);
        assert_eq!(second.pending_exit, BlockExit::ThreadReturn);
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
        let publications = copy_canonical_thread_publications_v1();
        assert!(matches!(
            publications.as_slice(),
            [CanonicalThreadPublicationV1::Returned { thread, .. }] if *thread == thread_id
        ));
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000), 0x1234);
        assert_eq!(copy_block_host_boundaries().len(), 2);
        assert_eq!(copy_block_execution_destinations().len(), 2);
        fn64_cpu_runtime::set_host_lookup(previous_host_lookup);
    }


    #[test]
    fn canonical_catalog_scheduler_reaches_a_one_instruction_limit() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let mut bytes = vec![0u8; 0x1008];
        let bank = BankId::new(0xca71);
        let entry = GuestPc::new(0x8000_0100);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, entry, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    bootstrap_return_runner,
                    ProgramArtifactIdentity::new([0x73; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, entry),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0x74; 32]),
        );
        let thread_id = 0xca71;

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(entry),
                thread_id,
                10,
            );
        }
        set_canonical_block_instruction_limit_v1(Some(1));

        assert!(crate::run_one_step());
        assert_eq!(canonical_block_charged_instructions_v1(), Some(1));
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(checkpoint)] = publications.as_slice() else {
            panic!("expected one exact one-instruction publication: {publications:?}");
        };
        assert_eq!(checkpoint.thread, thread_id);
        assert_eq!(checkpoint.charged_instructions, 1);
        assert_eq!(checkpoint.canonical_charged_instructions_at_publication, 1);
        assert_eq!(checkpoint.pending_exit, BlockExit::ThreadReturn);
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }


    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_dynamic_boot_observes_external_write_across_suspended_host_without_replay() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        DYNAMIC_BOOT_SOURCE_RUNS.store(0, Ordering::SeqCst);
        DYNAMIC_BOOT_HOST_RUNS.store(0, Ordering::SeqCst);
        DYNAMIC_BOOT_RESUME_RUNS.store(0, Ordering::SeqCst);

        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let resume = GuestPc::new(INSTALL_PC.get() + 0x18);
        let host_pc = GuestPc::new(INSTALL_PC.get() + 0x100);
        let mut bytes = vec![0u8; 0x8000];
        let jal = 0x0c00_0000 | ((host_pc.get() >> 2) & 0x03ff_ffff);
        put_physical_word(&mut bytes, dynamic_pc.get() & 0x1fff_ffff, jal);
        put_physical_word(
            &mut bytes,
            dynamic_pc.get().wrapping_add(4) & 0x1fff_ffff,
            0,
        );

        let bank = BankId::new(0xca85);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::from_spans(
                    bank,
                    vec![
                        CodeSpan::new(bank, INSTALL_PC, vec![0]).unwrap(),
                        CodeSpan::new(bank, resume, vec![0]).unwrap(),
                    ],
                )
                .unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    dynamic_boot_runner,
                    ProgramArtifactIdentity::new([0x85; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, INSTALL_PC),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(host_pc.get(), dynamic_boot_host)]).unwrap(),
            ProgramArtifactIdentity::new([0xe5; 32]),
        );
        let thread_id = 0xca85;

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_program_with_dynamic_mapped_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(INSTALL_PC),
                thread_id,
                10,
            );
        }
        set_canonical_block_instruction_limit_v1(Some(4));

        assert!(crate::run_one_step());
        assert_eq!(DYNAMIC_BOOT_SOURCE_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(DYNAMIC_BOOT_HOST_RUNS.load(Ordering::SeqCst), 0);
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(first)] = publications.as_slice() else {
            panic!("expected one exact dynamic checkpoint publication: {publications:?}");
        };
        assert_eq!(first.thread, thread_id);
        assert_eq!(first.charged_instructions, 3);
        assert_eq!(first.canonical_charged_instructions_at_publication, 3);
        let BlockExit::HostCall {
            vram,
            resume: dynamic_resume,
        } = first.pending_exit
        else {
            panic!("expected a dynamic host-call checkpoint: {first:?}");
        };
        assert_eq!(vram, host_pc);
        assert_eq!(dynamic_resume.pc, resume);
        assert!(crate::run_one_step());
        assert_eq!(DYNAMIC_BOOT_HOST_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(
            copy_canonical_thread_publications_v1(),
            vec![CanonicalThreadPublicationV1::OpaqueHostInFlight {
                thread: thread_id,
                target: host_pc,
                resume: dynamic_resume,
            }]
        );
        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        assert!(!rdram.is_null() && rdram_len > 0x7100);
        // SAFETY: guest execution is suspended, and this raw storage adapter
        // avoids creating a second mutable slice while the dormant coroutine
        // retains its checked `Rdram` view.
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        assert_eq!(
            unsafe { storage.read_u8(fn64_runtime::RdramAddr::from_offset(0x7100)) },
            1
        );

        // Model bytes already committed by an external producer. The typed PI
        // gateway is smoke-exercised here, but this plain catalog has no watched
        // mutation state, so real device timing and writer-journal ordering are
        // intentionally separate generation-backed contracts.
        unsafe {
            storage.write_u8(fn64_runtime::RdramAddr::from_offset(0x7100), 2);
        }
        fn64_cpu_runtime::notify_pi_dma_write(0x7100, 1);
        process_live_executable_writes_from_host();

        assert!(crate::run_one_step());
        assert!(matches!(
            copy_canonical_thread_publications_v1().as_slice(),
            [CanonicalThreadPublicationV1::OpaqueHostInFlight { thread, .. }]
                if *thread == thread_id
        ));
        assert!(crate::run_one_step());
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(second)] = publications.as_slice() else {
            panic!("expected one exact resumed checkpoint publication: {publications:?}");
        };
        assert_eq!(second.thread, thread_id);
        assert_eq!(second.charged_instructions, 1);
        assert_eq!(second.canonical_charged_instructions_at_publication, 4);
        assert_eq!(second.pending_exit, BlockExit::ThreadReturn);
        crate::run_to_idle();
        assert!(crate::is_thread_dead(thread_id));
        let publications = copy_canonical_thread_publications_v1();
        assert!(matches!(
            publications.as_slice(),
            [CanonicalThreadPublicationV1::Returned { thread, .. }] if *thread == thread_id
        ));
        assert_eq!(DYNAMIC_BOOT_SOURCE_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(DYNAMIC_BOOT_HOST_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(DYNAMIC_BOOT_RESUME_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(bytes[0x7100 ^ 3], 3);
        assert_eq!(copy_block_host_boundaries().len(), 2);
        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.aggregates.len(), 1);
        assert_eq!(telemetry.aggregates[0].charged_instructions, 2);
        assert_eq!(telemetry.dropped_identity_activations, 0);
        assert_eq!(canonical_block_charged_instructions_v1(), Some(4));
    }


    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_dynamic_generation_boot_orders_real_pi_dma_during_suspended_host() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        DYNAMIC_BOOT_SOURCE_RUNS.store(0, Ordering::SeqCst);
        DYNAMIC_BOOT_HOST_RUNS.store(0, Ordering::SeqCst);
        DYNAMIC_BOOT_RESUME_RUNS.store(0, Ordering::SeqCst);

        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let resume = GuestPc::new(INSTALL_PC.get() + 0x18);
        let host_pc = GuestPc::new(INSTALL_PC.get() + 0x100);
        let watched_bank = BankId::new(0xca86);
        let mut rom = vec![0u8; 0x21];
        rom[0x20] = 2;
        crate::load_rom_with_fixed_pi_latency(rom, 1);

        let mut bytes = vec![0u8; 0x8000];
        let jal = 0x0c00_0000 | ((host_pc.get() >> 2) & 0x03ff_ffff);
        put_physical_word(&mut bytes, dynamic_pc.get() & 0x1fff_ffff, jal);
        put_physical_word(
            &mut bytes,
            dynamic_pc.get().wrapping_add(4) & 0x1fff_ffff,
            0,
        );

        let bank = BankId::new(0xca85);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::from_spans(
                    bank,
                    vec![
                        CodeSpan::new(bank, INSTALL_PC, vec![0]).unwrap(),
                        CodeSpan::new(bank, resume, vec![0]).unwrap(),
                    ],
                )
                .unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    dynamic_boot_runner,
                    ProgramArtifactIdentity::new([0x85; 32]),
                ),
            )
            .unwrap();
        program
            .register(
                CodeBank::new(watched_bank, host_pc, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    watched_bank,
                    install_test_runner,
                    ProgramArtifactIdentity::new([0x86; 32]),
                ),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, INSTALL_PC),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(host_pc.get(), dynamic_boot_host)]).unwrap(),
            ProgramArtifactIdentity::new([0xe6; 32]),
        );
        let generation_id = GenerationId::new(0x86);
        let mut generation_catalog = PrecompiledGenerationCatalog::new();
        generation_catalog
            .register(
                PrecompiledGeneration::new(
                    generation_id,
                    host_pc,
                    GuestPc::new(host_pc.get() + 4),
                    host_pc,
                    GuestPc::new(host_pc.get() + 4),
                    sha2::Sha256::digest([0u8; 4]).into(),
                    vec![PrecompiledShard::new(
                        watched_bank,
                        host_pc,
                        GuestPc::new(host_pc.get() + 4),
                    )
                    .unwrap()],
                )
                .unwrap(),
            )
            .unwrap();
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            generation_catalog,
            vec![PrecompiledGenerationBackingV1::new(
                generation_id,
                vec![BackedExecutableSpanV1::new(host_pc, 0x7100, 4).unwrap()],
            )
            .unwrap()],
        )
        .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();
        let thread_id = 0xca86;

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_generation_program_with_dynamic_mapped_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(INSTALL_PC),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(DYNAMIC_BOOT_SOURCE_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(DYNAMIC_BOOT_HOST_RUNS.load(Ordering::SeqCst), 0);
        assert!(crate::run_one_step());
        assert_eq!(DYNAMIC_BOOT_HOST_RUNS.load(Ordering::SeqCst), 1);
        let prefix = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert_eq!(prefix.open_host_transactions.len(), 1);
        assert_eq!(prefix.entries.len(), 1);
        assert_eq!(
            prefix.entries[0].declared_writes[0].channel,
            WriterChannel::HostAbi
        );

        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        assert!(!rdram.is_null() && rdram_len > 0x7100);
        // SAFETY: the guest is suspended and the registered process allocation
        // remains live; use the raw adapter rather than aliasing its dormant
        // checked `Rdram` view.
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        assert_eq!(
            unsafe { storage.read_u8(fn64_runtime::RdramAddr::from_offset(0x7100)) },
            1
        );
        assert!(write_raw_mmio(0xffff_ffff_a460_0000, 0x7100));
        assert!(write_raw_mmio(0xffff_ffff_a460_0004, 0x20));
        assert!(write_raw_mmio(0xffff_ffff_a460_0008, 0));
        let pi_deadline = with_host(|host| {
            host.device_fabric
                .now()
                .get()
                .checked_add(1)
                .expect("PI completion deadline overflow")
        });
        crate::advance_virtual_time(pi_deadline);
        assert_eq!(
            unsafe { storage.read_u8(fn64_runtime::RdramAddr::from_offset(0x7100)) },
            2
        );
        let after_pi = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert_eq!(after_pi.open_host_transactions.len(), 1);
        assert_eq!(after_pi.entries.len(), 2);
        assert_eq!(
            after_pi.entries[1].declared_writes[0].channel,
            WriterChannel::PiDma
        );

        assert!(crate::run_one_step());
        crate::run_to_idle();
        assert!(crate::is_thread_dead(thread_id));
        assert_eq!(DYNAMIC_BOOT_SOURCE_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(DYNAMIC_BOOT_HOST_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(DYNAMIC_BOOT_RESUME_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(bytes[0x7100 ^ 3], 3);

        let evidence = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert!(evidence.open_host_transactions.is_empty());
        assert_eq!(
            evidence
                .entries
                .iter()
                .map(|entry| entry.declared_writes[0].channel)
                .collect::<Vec<_>>(),
            [
                WriterChannel::HostAbi,
                WriterChannel::PiDma,
                WriterChannel::HostAbi,
            ]
        );
        for entries in evidence.entries.windows(2) {
            assert_eq!(entries[0].after_sha256, entries[1].before_sha256);
        }
        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.aggregates.len(), 1);
        assert_eq!(telemetry.aggregates[0].charged_instructions, 2);
        assert_eq!(telemetry.dropped_identity_activations, 0);
        assert_eq!(canonical_block_charged_instructions_v1(), Some(4));
    }


    #[test]
    fn canonical_generation_boot_activates_explicit_physical_backing() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let mut bytes = vec![0u8; 0x1008];
        let image = [0x24, 0x02, 0x00, 0x01, 0x03, 0xe0, 0x00, 0x08];
        for (index, byte) in image.iter().copied().enumerate() {
            bytes[(0x80 + index) ^ 3] = byte;
        }
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(LIVE_BANK, LIVE_ENTRY, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    LIVE_BANK,
                    live_test_runner,
                    ProgramArtifactIdentity::new([0x73; 32]),
                ),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(LIVE_BANK, LIVE_ENTRY),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(LIVE_HOST.get(), live_host)]).unwrap(),
            ProgramArtifactIdentity::new([0x74; 32]),
        );
        let mut generation_catalog = PrecompiledGenerationCatalog::new();
        generation_catalog
            .register(
                PrecompiledGeneration::new(
                    GenerationId::new(0x75),
                    LIVE_ENTRY,
                    GuestPc::new(LIVE_ENTRY.get() + 8),
                    LIVE_ENTRY,
                    GuestPc::new(LIVE_ENTRY.get() + 8),
                    sha2::Sha256::digest(image).into(),
                    vec![PrecompiledShard::new(
                        LIVE_BANK,
                        LIVE_ENTRY,
                        GuestPc::new(LIVE_ENTRY.get() + 8),
                    )
                    .unwrap()],
                )
                .unwrap(),
            )
            .unwrap();
        let backed = BackedPrecompiledGenerationCatalogV1::new(
            generation_catalog,
            vec![PrecompiledGenerationBackingV1::new(
                GenerationId::new(0x75),
                vec![BackedExecutableSpanV1::new(LIVE_ENTRY, 0x80, 8).unwrap()],
            )
            .unwrap()],
        )
        .unwrap();
        let generation_install = CatalogGenerationInstallV1::new(resolver, backed).unwrap();
        let inactive_evidence = generation_install.evidence_snapshot();
        assert!(inactive_evidence.generations.active_segments.is_empty());
        let previous_host_lookup =
            fn64_cpu_runtime::set_host_lookup(Some(forbidden_catalog_legacy_lookup));
        let thread_id = 0xca76;

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_generation_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                generation_install,
                test_boot_context(LIVE_ENTRY),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        let active_evidence = catalog_generation_install_evidence_snapshot().unwrap();
        assert_eq!(active_evidence.resolver, inactive_evidence.resolver);
        assert_eq!(active_evidence.generations.active_segments.len(), 1);
        assert!(active_evidence.pending_physical_writes.is_empty());
        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000), 0x1234);
        fn64_cpu_runtime::set_host_lookup(previous_host_lookup);
    }
