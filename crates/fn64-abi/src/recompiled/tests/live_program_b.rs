use super::*;

    #[test]
    fn canonical_publication_binds_prepared_generation_continuation() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let generation_word = 0x2402_0091u32;
        let generation_image = generation_word.to_be_bytes();
        let mut bytes = vec![0u8; 0x7000];
        for (index, byte) in generation_image.iter().copied().enumerate() {
            bytes[(0x80 + index) ^ 3] = byte;
        }

        let mut program = BlockProgram::new();
        for (bank, entry, word, identity) in [
            (PREPARED_STATIC_BANK, PREPARED_STATIC_ENTRY, 0u32, 0x90),
            (
                PREPARED_GENERATION_BANK,
                PREPARED_GENERATION_ENTRY,
                generation_word,
                0x91,
            ),
        ] {
            program
                .register(
                    CodeBank::new(bank, entry, vec![word]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        prepared_generation_runner,
                        ProgramArtifactIdentity::new([identity; 32]),
                    ),
                )
                .unwrap();
        }
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(PREPARED_STATIC_BANK, PREPARED_STATIC_ENTRY),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0x92; 32]),
        );
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog
            .register(
                PrecompiledGeneration::new(
                    GenerationId::new(0x91),
                    PREPARED_GENERATION_ENTRY,
                    GuestPc::new(PREPARED_GENERATION_ENTRY.get() + 4),
                    PREPARED_GENERATION_ENTRY,
                    GuestPc::new(PREPARED_GENERATION_ENTRY.get() + 4),
                    sha2::Sha256::digest(generation_image).into(),
                    vec![PrecompiledShard::new(
                        PREPARED_GENERATION_BANK,
                        PREPARED_GENERATION_ENTRY,
                        GuestPc::new(PREPARED_GENERATION_ENTRY.get() + 4),
                    )
                    .unwrap()],
                )
                .unwrap(),
            )
            .unwrap();
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            catalog,
            vec![PrecompiledGenerationBackingV1::new(
                GenerationId::new(0x91),
                vec![BackedExecutableSpanV1::new(PREPARED_GENERATION_ENTRY, 0x80, 4).unwrap()],
            )
            .unwrap()],
        )
        .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();
        let thread_id = 0xca91;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_catalog_generation_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(PREPARED_STATIC_ENTRY),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(checkpoint)] = publications.as_slice() else {
            panic!("prepared generation did not publish an exact checkpoint: {publications:?}");
        };
        assert!(matches!(
            checkpoint.pending_exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::NoActiveGeneration,
                ..
            })
        ));
        assert_eq!(
            checkpoint.prepared_continuation,
            Some(CanonicalPreparedContinuationV1::InactiveGeneration {
                entry: ExecutionKey::new(PREPARED_GENERATION_BANK, PREPARED_GENERATION_ENTRY,),
            })
        );

        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }


    #[test]
    fn host_memory_api_write_is_journaled_as_host_abi() {
        // The point of the attributed host-memory API: a write from OUTSIDE the
        // guest still lands in the mutation journal with a declaring writer.
        // Writing the same bytes through `Rdram::as_mut_slice` declares
        // nothing, which is the hole this closes -- so this test fails if the
        // API ever stops bracketing its write in a child transaction.
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let image_a = 0x2402_0001u32.to_be_bytes();
        let image_b = 0x2402_0002u32.to_be_bytes();
        let mut bytes = vec![0u8; 0x6004];
        for (index, byte) in image_a.iter().copied().enumerate() {
            bytes[(0x80 + index) ^ 3] = byte;
        }
        let mut program = BlockProgram::new();
        for (bank, word, identity) in [
            (CATALOG_REWRITE_A, 0x2402_0001, 0x81),
            (CATALOG_REWRITE_B, 0x2402_0002, 0x82),
        ] {
            program
                .register(
                    CodeBank::new(bank, CATALOG_REWRITE_ENTRY, vec![word]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        catalog_rewrite_runner,
                        ProgramArtifactIdentity::new([identity; 32]),
                    ),
                )
                .unwrap();
        }
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(CATALOG_REWRITE_A, CATALOG_REWRITE_ENTRY),
                InstructionBudget::new(4).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0x83; 32]),
        );
        let mut catalog = PrecompiledGenerationCatalog::new();
        for (id, bank, image) in [
            (1, CATALOG_REWRITE_A, image_a),
            (2, CATALOG_REWRITE_B, image_b),
        ] {
            catalog
                .register(
                    PrecompiledGeneration::new(
                        GenerationId::new(id),
                        CATALOG_REWRITE_ENTRY,
                        GuestPc::new(CATALOG_REWRITE_ENTRY.get() + 4),
                        CATALOG_REWRITE_ENTRY,
                        GuestPc::new(CATALOG_REWRITE_ENTRY.get() + 4),
                        sha2::Sha256::digest(image).into(),
                        vec![PrecompiledShard::new(
                            bank,
                            CATALOG_REWRITE_ENTRY,
                            GuestPc::new(CATALOG_REWRITE_ENTRY.get() + 4),
                        )
                        .unwrap()],
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let backing = |id| {
            PrecompiledGenerationBackingV1::new(
                GenerationId::new(id),
                vec![BackedExecutableSpanV1::new(CATALOG_REWRITE_ENTRY, 0x80, 4).unwrap()],
            )
            .unwrap()
        };
        let generations =
            BackedPrecompiledGenerationCatalogV1::new(catalog, vec![backing(2), backing(1)])
                .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_generation_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(CATALOG_REWRITE_ENTRY),
                0xca83,
                10,
            );
        }

        let before = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap()
            .entries
            .len();

        // Write image_b over the watched executable span through the public API.
        assert!(
            crate::recompiled::write_guest_physical(0x80, &image_b),
            "attributed host write was refused"
        );
        process_live_executable_writes_from_host();

        let journal = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert_eq!(
            journal.entries.len(),
            before + 1,
            "host memory API write did not produce a journal entry"
        );
        let entry = journal.entries.last().unwrap();
        assert_eq!(
            entry.declared_writes[0].channel,
            WriterChannel::HostAbi,
            "host memory API write was not attributed to HostAbi"
        );
        assert!(
            entry
                .declared_writes
                .iter()
                .any(|declaration| declaration.physical_start <= 0x80
                    && declaration.physical_end >= 0x84),
            "declaration does not cover the bytes written: {:?}",
            entry.declared_writes
        );

        // And the bytes actually landed.
        assert_eq!(
            crate::recompiled::read_guest_physical(0x80, 4).unwrap(),
            image_b.to_vec(),
            "read_guest_physical did not observe the write"
        );
    }

    #[test]
    fn canonical_generation_cpu_write_retires_a_before_b_executes() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        let image_a = 0x2402_0001u32.to_be_bytes();
        let image_b = 0x2402_0002u32.to_be_bytes();
        let mut bytes = vec![0u8; 0x6004];
        for (index, byte) in image_a.iter().copied().enumerate() {
            bytes[(0x80 + index) ^ 3] = byte;
        }
        let mut program = BlockProgram::new();
        for (bank, word, identity) in [
            (CATALOG_REWRITE_A, 0x2402_0001, 0x81),
            (CATALOG_REWRITE_B, 0x2402_0002, 0x82),
        ] {
            program
                .register(
                    CodeBank::new(bank, CATALOG_REWRITE_ENTRY, vec![word]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        catalog_rewrite_runner,
                        ProgramArtifactIdentity::new([identity; 32]),
                    ),
                )
                .unwrap();
        }
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(CATALOG_REWRITE_A, CATALOG_REWRITE_ENTRY),
                InstructionBudget::new(4).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0x83; 32]),
        );
        let mut catalog = PrecompiledGenerationCatalog::new();
        for (id, bank, image) in [
            (1, CATALOG_REWRITE_A, image_a),
            (2, CATALOG_REWRITE_B, image_b),
        ] {
            catalog
                .register(
                    PrecompiledGeneration::new(
                        GenerationId::new(id),
                        CATALOG_REWRITE_ENTRY,
                        GuestPc::new(CATALOG_REWRITE_ENTRY.get() + 4),
                        CATALOG_REWRITE_ENTRY,
                        GuestPc::new(CATALOG_REWRITE_ENTRY.get() + 4),
                        sha2::Sha256::digest(image).into(),
                        vec![PrecompiledShard::new(
                            bank,
                            CATALOG_REWRITE_ENTRY,
                            GuestPc::new(CATALOG_REWRITE_ENTRY.get() + 4),
                        )
                        .unwrap()],
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let backing = |id| {
            PrecompiledGenerationBackingV1::new(
                GenerationId::new(id),
                vec![BackedExecutableSpanV1::new(CATALOG_REWRITE_ENTRY, 0x80, 4).unwrap()],
            )
            .unwrap()
        };
        let generations =
            BackedPrecompiledGenerationCatalogV1::new(catalog, vec![backing(2), backing(1)])
                .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();
        let thread_id = 0xca82;

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_generation_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(CATALOG_REWRITE_ENTRY),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        let after_cpu_write = catalog_generation_install_evidence_snapshot().unwrap();
        assert!(
            after_cpu_write.generations.active_segments.is_empty(),
            "generation A remained active across its committed executable write"
        );
        let cpu_journal = after_cpu_write.mutation_journal.unwrap();
        assert!(cpu_journal.sealed);
        assert_eq!(cpu_journal.entries.len(), 1);
        assert_eq!(
            cpu_journal.entries[0].declared_writes[0].channel,
            WriterChannel::CpuInstructionStore
        );
        assert_eq!(
            cpu_journal.entries[0].invalidated_generations,
            [GenerationId::new(1)]
        );
        assert_eq!(Rdram::new(&mut bytes).load_w(0xffff_ffff_8000_0010), 0);

        assert!(crate::run_one_step());
        let evidence = catalog_generation_install_evidence_snapshot().unwrap();
        assert_eq!(evidence.generations.active_segments.len(), 1);
        assert_eq!(
            evidence.generations.active_segments[0].generation,
            GenerationId::new(2)
        );
        assert_eq!(
            Rdram::new(&mut bytes).load_w(0xffff_ffff_8000_0010),
            0x0000_beef
        );
        fn64_cpu_runtime::notify_host_abi_write(0x80, 4);
        process_live_executable_writes_from_host();
        let after_host_write = catalog_generation_install_evidence_snapshot().unwrap();
        assert!(
            after_host_write.generations.active_segments.is_empty(),
            "host/DMA write notification did not retire generation B"
        );
        let host_journal = after_host_write.mutation_journal.unwrap();
        assert_eq!(host_journal.entries.len(), 2);
        assert_eq!(
            host_journal.entries[1].declared_writes[0].channel,
            WriterChannel::HostAbi
        );
        assert_eq!(
            host_journal.entries[1].invalidated_generations,
            [GenerationId::new(2)]
        );
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }


    #[test]
    fn interpreter_cpu_store_retires_generation_before_its_next_instruction() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().clear());
        REWRITE_B_ENTRIES.with(|entries| entries.borrow_mut().clear());
        let mut bytes = vec![0u8; 0x200];
        fn64_cpu_runtime::set_write_observer(None);
        fn64_cpu_runtime::set_guest_write_boundary_observer(None);
        {
            let mut mem = Rdram::new(&mut bytes);
            for (index, word) in REWRITE_A_WORDS.into_iter().enumerate() {
                mem.store_w(
                    0xFFFF_FFFF_8000_0000
                        | u64::from(REWRITE_PHYSICAL + u32::try_from(index * 4).unwrap()),
                    word,
                );
            }
        }
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(
            REWRITE_ENTRY,
            GuestPc::new(REWRITE_ENTRY.get() + u32::try_from(REWRITE_A_WORDS.len() * 4).unwrap()),
        );
        region
            .install(
                &mut program,
                CodeBank::new(REWRITE_OLD_BANK, REWRITE_ENTRY, REWRITE_A_WORDS.to_vec()).unwrap(),
                GeneratedBankRunner::new(REWRITE_OLD_BANK, rewrite_interpreter_runner),
            )
            .unwrap();
        let thread_id = 0xC0DE;

        // SAFETY: `bytes` remains live until this thread returns below.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(REWRITE_OLD_BANK, REWRITE_ENTRY),
                test_boot_context(REWRITE_ENTRY),
                rewrite_lookup,
                rewrite_transfer_lookup,
                InstructionBudget::new(13).unwrap(),
                thread_id,
                10,
            );
        }
        register_live_executable_region(
            REWRITE_PHYSICAL,
            REWRITE_PHYSICAL + u32::try_from(REWRITE_A_WORDS.len() * 4).unwrap(),
            region,
            rewrite_builder,
        );

        assert!(crate::run_one_step());
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0020) as u32, 0x55);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0024) as u32, 0x66);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_8000_0010) as u32,
            0,
            "generation A executed its post-store sentinel before invalidation"
        );
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0014) as u32, 0);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        assert!(live
            .program
            .borrow()
            .code()
            .bank(REWRITE_OLD_BANK)
            .is_none());
        assert!(live
            .program
            .borrow()
            .code()
            .bank(REWRITE_NEW_BANK)
            .is_some());
        assert_eq!(
            live.resolve_entry(REWRITE_ENTRY).unwrap().bank,
            REWRITE_NEW_BANK
        );
        assert_eq!(
            live.resolve_entry(REWRITE_RESUME).unwrap(),
            ExecutionKey::new(REWRITE_NEW_BANK, REWRITE_RESUME)
        );
        assert_eq!(
            REWRITE_BUILDS.with(|builds| builds.borrow().clone()),
            vec![(
                1,
                std::iter::once(0x1122_3344)
                    .chain(REWRITE_A_WORDS.into_iter().skip(1))
                    .flat_map(u32::to_be_bytes)
                    .collect::<Vec<_>>()
            )]
        );
        assert!(REWRITE_B_ENTRIES.with(|entries| entries.borrow().is_empty()));

        assert!(crate::run_one_step());
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0010) as u32, 0);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0014) as u32, 2);
        assert_eq!(
            REWRITE_B_ENTRIES.with(|entries| entries.borrow().clone()),
            vec![ExecutionKey::new(REWRITE_NEW_BANK, REWRITE_RESUME)]
        );
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }


    #[test]
    fn pi_dma_rebuilds_executable_region_before_completion_is_observable() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = crate::HostState::default());
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().clear());
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x28].copy_from_slice(&[0x3c, 0x08, 0x12, 0x34, 0x35, 0x08, 0x56, 0x78]);
        crate::load_rom_with_fixed_pi_latency(rom, 5);
        let mut bytes = vec![0u8; 0x200];
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(DMA_ENTRY, GuestPc::new(DMA_ENTRY.get() + 8));
        region
            .install(
                &mut program,
                CodeBank::new(DMA_OLD_BANK, DMA_ENTRY, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new(DMA_OLD_BANK, dma_rewrite_runner),
            )
            .unwrap();
        let thread_id = 0xD00D;

        // SAFETY: `bytes` remains live until this thread returns below.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(DMA_OLD_BANK, DMA_ENTRY),
                test_boot_context(DMA_ENTRY),
                dma_lookup,
                dma_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }
        register_live_executable_region(
            DMA_PHYSICAL,
            DMA_PHYSICAL + 8,
            region,
            dma_rewrite_builder,
        );

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 5);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        assert!(live.program.borrow().code().bank(DMA_OLD_BANK).is_none());
        assert_eq!(live.resolve_entry(DMA_ENTRY).unwrap().bank, DMA_NEW_BANK);
        assert_eq!(
            REWRITE_BUILDS.with(|builds| builds.borrow().clone()),
            vec![(1, vec![0x3c, 0x08, 0x12, 0x34, 0x35, 0x08, 0x56, 0x78])]
        );

        assert!(crate::run_one_step());
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0014) as u32, 0xD00D_0001);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_8000_0018) as u32,
            0xD00D_0002,
            "the already-serviced DMA boundary split generation B's first turn"
        );
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }


    #[test]
    fn fetch_activated_region_defers_dirty_image_until_attempted_fetch() {
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().clear());
        PENDING_EXECUTABLE_WRITES
            .with(|pending| *pending.borrow_mut() = vec![(0, 4), (DMA_PHYSICAL - 4, 16)]);
        let completed = [0x3c, 0x08, 0x12, 0x34, 0x35, 0x08, 0x56, 0x78];
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(DMA_ENTRY, GuestPc::new(DMA_ENTRY.get() + 8));
        region
            .install(
                &mut program,
                CodeBank::new(DMA_OLD_BANK, DMA_ENTRY, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new(DMA_OLD_BANK, dma_rewrite_runner),
            )
            .unwrap();
        let live = LiveBlockProgram {
            program: Rc::new(RefCell::new(program)),
            entry_lookup: dma_lookup,
            transfer_lookup: dma_transfer_lookup,
            budget: InstructionBudget::new(8).unwrap(),
            dispatch_artifact_identity: None,
            executable_regions: Rc::new(RefCell::new(vec![ObservedExecutableRegion {
                physical_start: DMA_PHYSICAL,
                physical_end: DMA_PHYSICAL + 8,
                region,
                next_generation: 1,
                builder: dma_rewrite_builder,
                builder_artifact_identity: None,
                activation: ExecutableActivation::FetchBoundary,
            }])),
            precompiled_generations: Rc::new(RefCell::new(None)),
        };

        assert!(process_executable_writes(&live, |offset| completed
            [usize::try_from(offset - DMA_PHYSICAL).unwrap()])
        .is_empty());
        assert!(REWRITE_BUILDS.with(|builds| builds.borrow().is_empty()));
        assert_eq!(
            PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
            vec![(DMA_PHYSICAL, 8)]
        );
        assert!(live.program.borrow().code().bank(DMA_OLD_BANK).is_some());

        let attempted = ExecutionKey::new(DMA_OLD_BANK, GuestPc::new(DMA_ENTRY.get() + 4));
        let retry = activate_fetch_generation(
            &live,
            attempted,
            AotMiss {
                expected_bank: DMA_OLD_BANK,
                va_start: DMA_ENTRY,
                byte_len: 8,
                expected_sha256: [0x11; 32],
                actual_sha256: [0x22; 32],
                first_diff_offset: None,
            },
            |offset| completed[usize::try_from(offset - DMA_PHYSICAL).unwrap()],
        )
        .unwrap();
        assert_eq!(retry, ExecutionKey::new(DMA_NEW_BANK, attempted.pc));
        assert!(live.program.borrow().code().bank(DMA_OLD_BANK).is_none());
        assert!(live.program.borrow().code().bank(DMA_NEW_BANK).is_some());
        assert_eq!(
            REWRITE_BUILDS.with(|builds| builds.borrow().clone()),
            vec![(1, completed.to_vec())]
        );
        assert!(PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().is_empty()));
    }
