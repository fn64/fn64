use super::*;

    #[test]
    fn canonical_mutation_state_traps_unjournaled_executable_bytes_before_dispatch() {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut image = [0u8; 8];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x100, 0x108)]);
        state.seal_with(|physical| image[(physical - 0x100) as usize]);
        image[3] = 0x5a;
        let snapshot = state.read_snapshot(|physical| image[(physical - 0x100) as usize]);

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.reconcile_snapshot_before_dispatch(snapshot);
        }))
        .expect_err("unjournaled executable mutation must trap");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains("unjournaled executable mutation"));
        assert!(message.contains("0x00000103"));
    }


    #[test]
    fn canonical_instruction_limit_clamps_the_final_dispatch_slice_exactly() {
        let _reset = PublicSiRuntimeStateTestReset;
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());

        let bank = BankId::new(0xc11a);
        let entry = GuestPc::new(0x8000_7000);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, entry, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    bootstrap_return_runner,
                    ProgramArtifactIdentity::new([0xc1; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, entry),
                InstructionBudget::new(4096).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xc2; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        let resolver_evidence = live.install.evidence().clone();

        assert_eq!(live.next_dispatch_budget().get(), 4096);
        set_canonical_block_instruction_limit_v1(Some(1720));
        assert_eq!(live.next_dispatch_budget().get(), 1720);
        assert_eq!(live.install.evidence(), &resolver_evidence);
        let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            set_canonical_block_instruction_limit_v1(Some(2000));
        }))
        .expect_err("an armed exact limit may not be replaced");
        let duplicate = duplicate
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| duplicate.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(duplicate.contains("already armed"));
        live.charge_canonical_instructions(1718);
        assert_eq!(live.next_dispatch_budget().get(), 2);
        live.charge_canonical_instructions(1);
        assert_eq!(live.next_dispatch_budget().get(), 1);

        set_canonical_block_instruction_limit_v1(None);
        assert_eq!(live.next_dispatch_budget().get(), 4096);
        set_canonical_block_instruction_limit_v1(Some(1720));
        assert_eq!(live.next_dispatch_budget().get(), 1);
        live.charge_canonical_instructions(1);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = live.next_dispatch_budget();
        }))
        .expect_err("dispatch may not continue past the exact limit");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains("limit 1720 was already reached"));
    }


    #[test]
    fn canonical_mutation_state_hash_chains_exact_channel_and_invalidation() {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut image = [0u8; 8];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x200, 0x208)]);
        state.seal_with(|physical| image[(physical - 0x200) as usize]);
        let initial_root = state.journal_root_sha256;
        image[2..4].copy_from_slice(&[0xaa, 0xbb]);
        let snapshot = state.read_snapshot(|physical| image[(physical - 0x200) as usize]);
        state.commit_snapshot(
            snapshot,
            vec![GuestWriteEvent::Range {
                channel: WriterChannel::HostAbi,
                physical_offset: 0x202,
                len: 2,
            }],
            vec![GenerationId::new(7)],
        );

        let evidence = state.evidence_snapshot();
        assert!(evidence.sealed);
        assert_ne!(evidence.journal_root_sha256, initial_root);
        assert_eq!(evidence.entries.len(), 1);
        let entry = &evidence.entries[0];
        assert_eq!(entry.sequence, 0);
        assert_eq!(
            entry.declared_writes,
            [AttributedExecutableWriteEvidenceV1 {
                channel: WriterChannel::HostAbi,
                physical_start: 0x202,
                physical_end: 0x204,
            }]
        );
        assert_eq!(
            entry.changed_ranges,
            [PendingExecutableWriteEvidenceSnapshot {
                physical_start: 0x202,
                physical_end: 0x204,
            }]
        );
        assert_eq!(entry.invalidated_generations, [GenerationId::new(7)]);
        let stable = state.read_snapshot(|physical| image[(physical - 0x200) as usize]);
        state.reconcile_snapshot_before_dispatch(stable);
    }


    #[test]
    fn canonical_mutation_state_rejects_changes_outside_attributed_range() {
        let mut image = [0u8; 8];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x300, 0x308)]);
        state.seal_with(|physical| image[(physical - 0x300) as usize]);
        image[6] = 1;
        let snapshot = state.read_snapshot(|physical| image[(physical - 0x300) as usize]);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.commit_snapshot(
                snapshot,
                vec![GuestWriteEvent::Range {
                    channel: WriterChannel::RdpRenderer,
                    physical_offset: 0x300,
                    len: 2,
                }],
                Vec::new(),
            );
        }))
        .expect_err("out-of-declaration executable change must trap");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains("outside every attributed writer declaration"));
    }


    #[test]
    fn renderer_transaction_attributes_exact_changed_executable_bytes() {
        let _state = scoped_test_executable_write_preflight_state(vec![(0x40, 0x48)], Vec::new());
        let previous =
            fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));
        let mut storage = [0u8; 0x80];
        track_rdp_renderer_mutation(&mut storage, |storage| {
            storage[0x41 ^ 3] = 0xaa;
            storage[0x42 ^ 3] = 0xbb;
            storage[0x70 ^ 3] = 0xcc;
        });
        fn64_recomp_rs::set_write_observer(previous);

        assert_eq!(
            PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
            [GuestWriteEvent::Range {
                channel: WriterChannel::RdpRenderer,
                physical_offset: 0x41,
                len: 2,
            }]
        );
    }


    /// The renderer tracker must watch the ranges the GUARD checks, not just
    /// this thread's registered write ranges.
    ///
    /// These two sets normally agree -- the canonical fixture mirrors the
    /// watched ranges into EXECUTABLE_WRITE_RANGES -- which is exactly why the
    /// divergence went unnoticed. When they differ, a renderer write to
    /// executable bytes the guard watches but the thread-local set omits was
    /// snapshotted by nobody and notified by nobody, then surfaced at the next
    /// commit as an undeclared mutation with events=0 declarations=0. WM2000
    /// patching a store immediate at 0x8009b0b0 during a graphics task is that
    /// case.
    #[test]
    fn renderer_tracker_watches_guard_ranges_not_just_thread_local_ranges() {
        // The guard watches [0x40,0x48); the thread-local set deliberately
        // does NOT cover 0x44.
        let _state = scoped_test_executable_write_preflight_state(vec![(0x40, 0x42)], Vec::new());
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x40, 0x48)]);
        let mut storage = [0u8; 0x80];
        state.seal_with(|physical| storage[((physical - 0x40) as usize) ^ 3]);

        let watched = state.watched_ranges();

        assert_eq!(
            watched,
            [(0x40, 0x48)],
            "the guard's watched set is what commit_snapshot compares against"
        );
        assert_ne!(
            watched,
            EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow().clone()),
            "this test is only meaningful while the two sets differ"
        );
        assert!(
            watched
                .iter()
                .any(|(start, end)| *start <= 0x44 && 0x44 < *end),
            "0x44 must be inside the guard's watched set"
        );
        assert!(
            !EXECUTABLE_WRITE_RANGES
                .with(|ranges| ranges.borrow().clone())
                .iter()
                .any(|(start, end)| *start <= 0x44 && 0x44 < *end),
            "0x44 must be outside the thread-local set, or the bug cannot occur"
        );
    }

    #[test]
    fn same_byte_nested_writers_commit_in_execution_order() {
        let mut image = [0u8; 8];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x400, 0x408)]);
        state.seal_with(|physical| image[(physical - 0x400) as usize]);
        let transaction = state.begin_host_transaction(
            7,
            GuestPc::new(0x8000_0400),
            ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_0404)),
        );

        for (value, channel) in [
            (1, WriterChannel::HostAbi),
            (2, WriterChannel::RspExecutionOrHleWriteback),
            (3, WriterChannel::RdpRenderer),
            (4, WriterChannel::HostAbi),
        ] {
            image[1] = value;
            let snapshot = state.read_snapshot(|physical| image[(physical - 0x400) as usize]);
            state.commit_snapshot(
                snapshot,
                vec![GuestWriteEvent::Range {
                    channel,
                    physical_offset: 0x401,
                    len: 1,
                }],
                Vec::new(),
            );
        }
        state.finish_host_transaction(transaction);

        let evidence = state.evidence_snapshot();
        assert!(evidence.open_host_transactions.is_empty());
        assert_eq!(evidence.entries.len(), 4);
        assert_eq!(
            evidence
                .entries
                .iter()
                .map(|entry| entry.declared_writes[0].channel)
                .collect::<Vec<_>>(),
            [
                WriterChannel::HostAbi,
                WriterChannel::RspExecutionOrHleWriteback,
                WriterChannel::RdpRenderer,
                WriterChannel::HostAbi,
            ]
        );
        for entries in evidence.entries.windows(2) {
            assert_eq!(entries[0].after_sha256, entries[1].before_sha256);
        }
    }


    #[test]
    fn catalog_host_orders_real_rsp_and_rdp_wrappers_on_the_same_byte() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let words = [0x2402_0001u32, 0x03e0_0008];
        let rom = words
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        crate::load_rom_with_fixed_pi_latency(rom.clone(), 1);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(ORDERED_SYNC_BANK, ORDERED_SYNC_ENTRY, words.to_vec()).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    ORDERED_SYNC_BANK,
                    ordered_sync_runner,
                    ProgramArtifactIdentity::new([0xaf; 32]),
                ),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(ORDERED_SYNC_BANK, ORDERED_SYNC_ENTRY),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(ORDERED_SYNC_HOST.get(), ordered_sync_host)]).unwrap(),
            ProgramArtifactIdentity::new([0xb0; 32]),
        );
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            PrecompiledGenerationCatalog::new(),
            Vec::new(),
        )
        .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();
        let mut bootstrap = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        bootstrap
            .publish_resident_rom_image(0, ORDERED_SYNC_ENTRY.get(), 8)
            .unwrap();
        let validated = bootstrap.commit().unwrap();
        boot_thread0_validated_catalog_generation_program_v1(
            validated,
            install,
            test_boot_context(ORDERED_SYNC_ENTRY),
            0x0adf,
            10,
        )
        .unwrap();

        assert!(crate::run_one_step());
        crate::run_to_idle();
        let evidence = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert!(evidence.open_host_transactions.is_empty());
        assert_eq!(
            evidence
                .entries
                .iter()
                .skip(1)
                .map(|entry| entry.declared_writes[0].channel)
                .collect::<Vec<_>>(),
            [
                WriterChannel::HostAbi,
                WriterChannel::RspExecutionOrHleWriteback,
                WriterChannel::RdpRenderer,
                WriterChannel::HostAbi,
            ]
        );
        for entry in evidence.entries.iter().skip(1) {
            assert_eq!(
                entry.changed_ranges,
                [PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x7200,
                    physical_end: 0x7201,
                }]
            );
        }
        for entries in evidence.entries.windows(2) {
            assert_eq!(entries[0].after_sha256, entries[1].before_sha256);
        }
    }


    #[test]
    fn suspended_host_transaction_orders_same_byte_device_write_before_resume_suffix() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let words = [0x2402_0001u32, 0x03e0_0008];
        let rom = words
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        crate::load_rom_with_fixed_pi_latency(rom.clone(), 1);

        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(ORDERED_WRITER_BANK, ORDERED_WRITER_ENTRY, words.to_vec()).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    ORDERED_WRITER_BANK,
                    ordered_writer_runner,
                    ProgramArtifactIdentity::new([0xad; 32]),
                ),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(ORDERED_WRITER_BANK, ORDERED_WRITER_ENTRY),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(ORDERED_WRITER_HOST.get(), ordered_writer_host)])
                .unwrap(),
            ProgramArtifactIdentity::new([0xae; 32]),
        );
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            PrecompiledGenerationCatalog::new(),
            Vec::new(),
        )
        .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();
        let mut bootstrap = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        bootstrap
            .publish_resident_rom_image(0, ORDERED_WRITER_ENTRY.get(), 8)
            .unwrap();
        let validated = bootstrap.commit().unwrap();
        let thread_id = 0x0ade;
        boot_thread0_validated_catalog_generation_program_v1(
            validated,
            install,
            test_boot_context(ORDERED_WRITER_ENTRY),
            thread_id,
            10,
        )
        .unwrap();

        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        let prefix = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert_eq!(prefix.open_host_transactions.len(), 1);
        assert_eq!(
            prefix.entries.last().unwrap().declared_writes[0].channel,
            WriterChannel::HostAbi
        );

        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        assert!(!rdram.is_null() && rdram_len > 0x7000);
        unsafe {
            fn64_runtime::RdramPtr::from_storage_ptr(rdram)
                .write_u8(fn64_runtime::RdramAddr::from_offset(0x7000), 2);
        }
        fn64_recomp_rs::notify_pi_dma_write(0x7000, 1);
        process_live_executable_writes_from_host();

        assert!(crate::run_one_step());
        crate::run_to_idle();

        let evidence = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert!(evidence.open_host_transactions.is_empty());
        let channels = evidence
            .entries
            .iter()
            .skip(1)
            .map(|entry| entry.declared_writes[0].channel)
            .collect::<Vec<_>>();
        assert_eq!(
            channels,
            [
                WriterChannel::HostAbi,
                WriterChannel::PiDma,
                WriterChannel::HostAbi
            ]
        );
        for entries in evidence.entries.windows(2) {
            assert_eq!(entries[0].after_sha256, entries[1].before_sha256);
        }

        let storage = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            track_rdp_renderer_mutation(&mut *storage, |_| {
                panic!("synthetic renderer unwind");
            });
        }))
        .expect_err("uncommitted child writer must unwind");
        assert!(unwind
            .downcast_ref::<&str>()
            .is_some_and(|message| *message == "synthetic renderer unwind"));

        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = begin_catalog_nested_writer(&*storage, "post-unwind publication");
        }))
        .expect_err("a later child writer must reject the poisoned owner");
        let message = poisoned
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| poisoned.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains(
            "canonical executable mutation owner is poisoned: tracked renderer/RSP publication child writer transaction unwound before commit"
        ));
    }
