    use super::*;
    use fn64_recomp_rs::{
        run_bank, BackedExecutableSpanV1, BlockRun, BootCicIdentity, BootCop0Context, BootRegion,
        CodeBank, CodeCatalog, CodeSpan, CpuFaultKind, GeneratedBankRunner, GenerationId,
        PhysicalCodeBank, PrecompiledGeneration, PrecompiledGenerationBackingV1, PrecompiledShard,
        Sha256Digest, BOOT_CONTEXT_SCHEMA_V1,
    };
    use sha2::Digest;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    static TRANSIENT_FR_SHIM_ENTERED: AtomicBool = AtomicBool::new(false);
    #[cfg(feature = "dynamic-mapped-runtime")]
    static DYNAMIC_BOOT_SOURCE_RUNS: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "dynamic-mapped-runtime")]
    static DYNAMIC_BOOT_HOST_RUNS: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "dynamic-mapped-runtime")]
    static DYNAMIC_BOOT_RESUME_RUNS: AtomicUsize = AtomicUsize::new(0);

    const INSTALL_BANK: BankId = BankId::new(0xb007);
    const INSTALL_PC: GuestPc = GuestPc::new(0x8000_7000);

    fn install_test_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn unified_transition_test_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let static_resume = GuestPc::new(INSTALL_PC.get() + 0x20);
        if entry.pc == INSTALL_PC {
            ctx.set_r32(2, ctx.r_u32(2).wrapping_add(1) as i32);
            return BlockRun::new(
                BlockExit::ResolveTransfer {
                    source_bank: entry.bank,
                    target_pc: dynamic_pc,
                },
                1,
            );
        }
        assert_eq!(entry.pc, static_resume);
        ctx.set_r32(3, ctx.r_u32(3).wrapping_add(1) as i32);
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn exact_withhold_normal_budget_runner(
        entry: ExecutionKey,
        budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            INSTALL_PC => panic!("withheld canonical entry executed statically"),
            pc if pc == GuestPc::new(INSTALL_PC.get() + 4) => {
                assert_eq!(
                    budget.get(),
                    7,
                    "one-shot withholding kept static slicing armed"
                );
                BlockRun::new(BlockExit::Yield(entry), 1)
            }
            pc => panic!("unexpected exact-withhold test PC {pc}"),
        }
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn unified_host_precedence_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        ctx.set_r32(2, ctx.r_u32(2).wrapping_add(1) as i32);
        BlockRun::new(
            BlockExit::ResolveCall {
                source_bank: entry.bank,
                target_pc: GuestPc::new(INSTALL_PC.get() + 0x10),
                resume: ExecutionKey::new(entry.bank, GuestPc::new(INSTALL_PC.get() + 0x20)),
            },
            1,
        )
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn unified_tlb_fault_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        ctx.set_r32(2, ctx.r_u32(2).wrapping_add(1) as i32);
        BlockRun::new(
            BlockExit::ResolveTransfer {
                source_bank: entry.bank,
                target_pc: GuestPc::new(0x0040_0000),
            },
            1,
        )
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn unified_dynamic_writer_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let static_resume = GuestPc::new(INSTALL_PC.get() + 0x40);
        if entry.pc == INSTALL_PC {
            return BlockRun::new(
                BlockExit::ResolveTransfer {
                    source_bank: entry.bank,
                    target_pc: dynamic_pc,
                },
                1,
            );
        }
        assert_eq!(entry.pc, static_resume);
        ctx.set_r32(3, ctx.r_u32(3).wrapping_add(1) as i32);
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn unified_executable_write_boundary_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let resume = GuestPc::new(INSTALL_PC.get() + 4);
        match entry.pc {
            INSTALL_PC => BlockRun::new(
                BlockExit::ExecutableWrite {
                    source_bank: entry.bank,
                    resume: ExecutionKey::new(entry.bank, resume),
                },
                1,
            ),
            pc if pc == resume => {
                ctx.set_r32(2, ctx.r_u32(2).wrapping_add(1) as i32);
                BlockRun::new(BlockExit::ThreadReturn, 1)
            }
            pc => panic!("unexpected unified executable-write test PC {pc}"),
        }
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn put_physical_word(storage: &mut [u8], physical: u32, word: u32) {
        for (offset, byte) in word.to_be_bytes().into_iter().enumerate() {
            storage[(physical as usize + offset) ^ 3] = byte;
        }
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn dynamic_boot_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let resume = GuestPc::new(INSTALL_PC.get() + 0x18);
        match entry.pc {
            INSTALL_PC => {
                DYNAMIC_BOOT_SOURCE_RUNS.fetch_add(1, Ordering::SeqCst);
                BlockRun::new(
                    BlockExit::ResolveTransfer {
                        source_bank: entry.bank,
                        target_pc: dynamic_pc,
                    },
                    1,
                )
            }
            pc if pc == resume => {
                DYNAMIC_BOOT_RESUME_RUNS.fetch_add(1, Ordering::SeqCst);
                BlockRun::new(BlockExit::ThreadReturn, 1)
            }
            pc => panic!("unexpected dynamic boot test PC {pc}"),
        }
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn dynamic_boot_host(_ctx: &mut RsContext, mem: &mut Rdram<'_>) {
        DYNAMIC_BOOT_HOST_RUNS.fetch_add(1, Ordering::SeqCst);
        mem.as_mut_slice()[0x7100 ^ 3] = 1;
        super::super::suspend_active_coroutine(fn64_runtime::Yield::PauseSelf);
        assert_eq!(
            mem.as_mut_slice()[0x7100 ^ 3],
            2,
            "external write was not visible before dynamic host resume"
        );
        mem.as_mut_slice()[0x7100 ^ 3] = 3;
    }

    fn bootstrap_return_runner(
        _entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        BlockRun::new(BlockExit::ThreadReturn, 1)
    }

    fn install_test_host(_ctx: &mut RsContext, _mem: &mut Rdram<'_>) {}

    fn alternate_install_test_host(_ctx: &mut RsContext, _mem: &mut Rdram<'_>) {}

    fn install_test_legacy_host_lookup(target: u32) -> Option<RecompFunc> {
        (target == INSTALL_PC.get() + 4).then_some(alternate_install_test_host)
    }

    fn install_test_function_lookup(_target: u32) -> RecompFunc {
        install_test_host
    }

    fn install_test_entry_lookup(target: GuestPc) -> Result<ExecutionKey, CpuFault> {
        Ok(ExecutionKey::new(BankId::new(0xcaff), target))
    }

    fn install_test_transfer_lookup(
        source_bank: BankId,
        target: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        Ok(ExecutionKey::new(source_bank, target))
    }

    fn install_test_program(bank: BankId, artifact_byte: u8) -> CatalogBlockProgramV1 {
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, INSTALL_PC, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    install_test_runner,
                    ProgramArtifactIdentity::new([artifact_byte; 32]),
                ),
            )
            .unwrap();
        CatalogBlockProgramV1::new(
            program,
            ExecutionKey::new(bank, INSTALL_PC),
            InstructionBudget::new(2).unwrap(),
        )
        .unwrap()
    }

    fn bootstrap_test_install(expected_word: u32) -> CatalogGenerationInstallV1 {
        let bank = INSTALL_BANK;
        let entry = GuestPc::new(0x8000_7000);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, entry, vec![expected_word]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    bootstrap_return_runner,
                    ProgramArtifactIdentity::new([0xb0; 32]),
                ),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, entry),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xb1; 32]),
        );
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            PrecompiledGenerationCatalog::new(),
            Vec::new(),
        )
        .unwrap();
        CatalogGenerationInstallV1::new(resolver, generations).unwrap()
    }

    fn bootstrap_test_install_with_additional_banks(
        entry_word: u32,
        static_word: u32,
        physical_word: u32,
    ) -> CatalogGenerationInstallV1 {
        let entry_bank = BankId::new(0xb007);
        let static_bank = BankId::new(0xb008);
        let physical_bank = BankId::new(0xb009);
        let entry = GuestPc::new(0x8000_7000);
        let static_pc = GuestPc::new(0x8000_8000);
        let mut program = BlockProgram::new();
        for (bank, pc, word, artifact_byte) in [
            (entry_bank, entry, entry_word, 0xb0),
            (static_bank, static_pc, static_word, 0xb2),
        ] {
            program
                .register(
                    CodeBank::new(bank, pc, vec![word]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        bootstrap_return_runner,
                        ProgramArtifactIdentity::new([artifact_byte; 32]),
                    ),
                )
                .unwrap();
        }
        program
            .register_physical_code(
                PhysicalCodeBank::new(physical_bank, 0x9000, vec![physical_word]).unwrap(),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(entry_bank, entry),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xb1; 32]),
        );
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            PrecompiledGenerationCatalog::new(),
            Vec::new(),
        )
        .unwrap();
        CatalogGenerationInstallV1::new(resolver, generations).unwrap()
    }

    fn bootstrap_test_install_with_generation(
        entry_word: u32,
        generation_word: u32,
    ) -> CatalogGenerationInstallV1 {
        let entry_bank = BankId::new(0xb007);
        let generation_bank = BankId::new(0xb00a);
        let entry = GuestPc::new(0x8000_7000);
        let generation_start = GuestPc::new(0x8000_a000);
        let generation_end = GuestPc::new(0x8000_a004);
        let mut program = BlockProgram::new();
        for (bank, pc, word, artifact_byte) in [
            (entry_bank, entry, entry_word, 0xb0),
            (generation_bank, generation_start, generation_word, 0xba),
        ] {
            program
                .register(
                    CodeBank::new(bank, pc, vec![word]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        bootstrap_return_runner,
                        ProgramArtifactIdentity::new([artifact_byte; 32]),
                    ),
                )
                .unwrap();
        }
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(entry_bank, entry),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xb1; 32]),
        );
        let generation_id = GenerationId::new(0xaaa);
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog
            .register(
                PrecompiledGeneration::new(
                    generation_id,
                    generation_start,
                    generation_end,
                    generation_start,
                    generation_end,
                    sha2::Sha256::digest(generation_word.to_be_bytes()).into(),
                    vec![
                        PrecompiledShard::new(generation_bank, generation_start, generation_end)
                            .unwrap(),
                    ],
                )
                .unwrap(),
            )
            .unwrap();
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            catalog,
            vec![PrecompiledGenerationBackingV1::new(
                generation_id,
                vec![BackedExecutableSpanV1::new(generation_start, 0xa000, 4).unwrap()],
            )
            .unwrap()],
        )
        .unwrap();
        CatalogGenerationInstallV1::new(resolver, generations).unwrap()
    }

    fn bootstrap_test_rdram_len() -> usize {
        fn64_recomp_rs::RDRAM_LEN
    }

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

    fn pi_test_trace(direction: fn64_runtime::DmaDirection) -> Vec<fn64_runtime::DeviceTraceEvent> {
        pi_test_trace_for_device(direction, fn64_runtime::PiDeviceAddress::RomOffset(0x20))
    }

    fn pi_test_trace_for_device(
        direction: fn64_runtime::DmaDirection,
        device: fn64_runtime::PiDeviceAddress,
    ) -> Vec<fn64_runtime::DeviceTraceEvent> {
        let request = fn64_runtime::PiDmaRequest {
            direction,
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6000),
            device,
            len: 4,
        };
        let completion = fn64_runtime::DmaCompletion {
            direction,
            dram_addr: request.dram_addr,
            device: request.device,
            len: request.len,
        };
        [
            fn64_runtime::DeviceTraceKind::PiDmaStarted(request),
            fn64_runtime::DeviceTraceKind::PiBytesCommitted(request),
            fn64_runtime::DeviceTraceKind::PiBusyCleared,
            fn64_runtime::DeviceTraceKind::MiInterruptRaised(fn64_runtime::InterruptSource::Pi),
            fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::PiDmaComplete(completion),
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(sequence, kind)| fn64_runtime::DeviceTraceEvent {
            at: fn64_runtime::Cycles::new(100 + sequence as u64),
            sequence: sequence as u64,
            kind,
        })
        .collect()
    }

    fn si_test_trace(kind: fn64_runtime::SiDmaKind) -> Vec<fn64_runtime::DeviceTraceEvent> {
        let request = fn64_runtime::SiDmaRequest {
            kind,
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x7000),
        };
        [
            fn64_runtime::DeviceTraceKind::SiDmaStarted(request),
            fn64_runtime::DeviceTraceKind::SiBytesCommitted(request),
            fn64_runtime::DeviceTraceKind::SiBusyCleared,
            fn64_runtime::DeviceTraceKind::MiInterruptRaised(fn64_runtime::InterruptSource::Si),
            fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::SiDmaComplete(request),
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(sequence, kind)| fn64_runtime::DeviceTraceEvent {
            at: fn64_runtime::Cycles::new(100 + sequence as u64),
            sequence: sequence as u64,
            kind,
        })
        .collect()
    }

    fn sp_test_trace(
        direction: fn64_runtime::SpDmaDirection,
    ) -> Vec<fn64_runtime::DeviceTraceEvent> {
        let request = fn64_runtime::SpDmaRequest {
            direction,
            mem_addr: fn64_runtime::RspMemAddr::from_register(0),
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6000),
            encoded_len: 7,
        };
        [
            fn64_runtime::DeviceTraceKind::SpDmaStarted(request),
            fn64_runtime::DeviceTraceKind::SpDmaBytesCommitted(request),
            fn64_runtime::DeviceTraceKind::SpDmaBusyCleared,
        ]
        .into_iter()
        .enumerate()
        .map(|(sequence, kind)| fn64_runtime::DeviceTraceEvent {
            at: fn64_runtime::Cycles::new(100 + sequence as u64),
            sequence: sequence as u64,
            kind,
        })
        .collect()
    }

    fn production_aot_receipt_for_si_test() -> StaticExecutionBuildReceipt {
        StaticExecutionBuildReceipt {
            schema: 1,
            aot_runtime: true,
            production_aot: true,
            dev_interpreter: false,
        }
    }

    fn rdp_renderer_validator_fixture(
        publications: Vec<Vec<u64>>,
    ) -> (
        [u8; 0x80],
        CanonicalExecutableMutationStateV1,
        RdpRendererWriterRuntimeTraceEpochV1,
        RdpRendererWriterTraceV1,
    ) {
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut storage = [0u8; 0x80];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x40, 0x48)]);
        state.seal_with(|_| 0);
        storage[0x41 ^ 3] = 0xa5;
        let view = fn64_runtime::RdramView::from_storage(&storage);
        let snapshot = state
            .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
        state.commit_snapshot(
            snapshot,
            vec![GuestWriteEvent::Range {
                channel: WriterChannel::RdpRenderer,
                physical_offset: 0x41,
                len: 1,
            }],
            Vec::new(),
        );
        let epoch = RdpRendererWriterRuntimeTraceEpochV1 {
            epoch_id: 0x71,
            program_model_sha256: [0x72; 32],
        };
        let trace = RdpRendererWriterTraceV1 {
            epoch_id: epoch.epoch_id,
            program_model_sha256: epoch.program_model_sha256,
            initial_journal_entry_count: 0,
            next_journal_entry_index: state.entries.len(),
            publications,
            rejected_journal_sequences: Vec::new(),
        };
        (storage, state, epoch, trace)
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

    struct PublicSiRuntimeStateTestReset;

    impl Drop for PublicSiRuntimeStateTestReset {
        fn drop(&mut self) {
            with_executor(|executor| *executor = fn64_runtime::Executor::new());
            with_host(|host| *host = super::super::HostState::default());
            PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
            PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
            EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow_mut().clear());
            CPU_INSTRUCTION_STORE_TRACE.with(|trace| *trace.borrow_mut() = None);
            RDP_RENDERER_WRITER_TRACE.with(|trace| *trace.borrow_mut() = None);
            fn64_recomp_rs::set_write_observer(None);
            fn64_recomp_rs::set_guest_write_boundary_observer(None);
        }
    }

    fn install_public_si_runtime_state_test_owner() -> fn64_runtime::SiDmaRequest {
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
        // Unit tests compile the development feature lane. Override only this
        // private test owner so the public runtime-state path can exercise its
        // production-lane precondition without weakening the real constructor.
        resolver.evidence.build_receipt = production_aot_receipt_for_si_test();
        let bootstrap_evidence = validated.receipt().evidence().clone();
        let watched_ranges = bootstrap_evidence.watched_ranges.clone();
        let writer_program_model_sha256 =
            canonical_writer_program_model_sha256(&resolver, Some(&generations), &watched_ranges);
        let state = CanonicalExecutableMutationStateV1::from_bootstrap(
            &bootstrap_evidence,
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
            bootstrap_evidence: Some(bootstrap_evidence),
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
        let mut storage = validated.storage;
        let rdram = storage.as_mut_ptr();
        let rdram_len = storage.len();
        with_host(|host| {
            *host = super::super::HostState::default();
            host.runtime_rdram = rdram;
            host.runtime_rdram_len = rdram_len;
            host.owned_runtime_rdram = Some(storage);
            host.canonical_recompiled_program = Some(live);
        });
        EXECUTABLE_WRITE_RANGES.with(|ranges| {
            ranges.borrow_mut().extend(
                watched_ranges
                    .iter()
                    .map(|range| (range.physical_start, range.physical_end)),
            );
        });
        fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));
        fn64_recomp_rs::set_guest_write_boundary_observer(Some(classify_live_executable_write));
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        crate::load_rom(rom);
        fn64_runtime::SiDmaRequest {
            kind: fn64_runtime::SiDmaKind::PifToDram,
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6000),
        }
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
                at: fn64_runtime::Cycles::new(200 + sequence),
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
                at: fn64_runtime::Cycles::new(100 + sequence as u64),
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
            physical_offset: fn64_recomp_rs::RDRAM_LEN as u32,
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
                at: fn64_runtime::Cycles::new(100 + sequence as u64),
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
        nonmonotonic[3].at = fn64_runtime::Cycles::new(99);
        assert_eq!(
            validate_si_transition_trace(&nonmonotonic).unwrap_err(),
            SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder
        );
        let mut sequence_regression = si_test_trace(fn64_runtime::SiDmaKind::PifToDram);
        sequence_regression[3].at = fn64_runtime::Cycles::new(200);
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

    #[test]
    fn validated_boot_owns_rdram_and_starts_journal_with_bootstrap_batch() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom(rom.clone());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let entry = GuestPc::new(0x8000_7000);

        boot_thread0_validated_catalog_generation_program_v1(
            validated,
            install,
            test_boot_context(entry),
            0xb007,
            10,
        )
        .unwrap();

        let evidence = catalog_generation_install_evidence_snapshot().unwrap();
        assert!(evidence.bootstrap.is_some());
        let journal = evidence.mutation_journal.unwrap();
        assert!(journal.sealed);
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].sequence, 0);
        assert!(journal.entries[0]
            .declared_writes
            .iter()
            .all(|write| write.channel == WriterChannel::BootstrapOrImport));
        let completion = take_validated_bootstrap_writer_channel_receipt_v1()
            .expect("validated boot must mint bootstrap writer authority");
        assert!(completion.has_valid_evidence_hash());
        assert_eq!(completion.evidence().journal_entry, journal.entries[0]);
        assert!(take_validated_bootstrap_writer_channel_receipt_v1().is_none());
        let mut steps = 0;
        while !crate::is_thread_dead(0xb007) {
            assert!(
                crate::run_one_step(),
                "validated bootstrap thread stalled before returning"
            );
            steps += 1;
            assert!(steps < 4, "validated bootstrap thread did not return");
        }
        assert!(crate::is_thread_dead(0xb007));
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
    }

    #[test]
    fn canonical_scheduler_mirror_commits_exact_host_abi_write_before_dispatch() {
        let _reset = PublicSiRuntimeStateTestReset;
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());

        let entry_word = 0x2402_0001;
        let static_word = 0;
        let physical_word = 0x2404_0003;
        let install =
            bootstrap_test_install_with_additional_banks(entry_word, static_word, physical_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&entry_word.to_be_bytes());
        rom[0x24..0x28].copy_from_slice(&static_word.to_be_bytes());
        rom[0x28..0x2c].copy_from_slice(&physical_word.to_be_bytes());
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom(rom.clone());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        for (rom_start, vram_start) in [
            (0x20, 0x8000_7000),
            (0x24, 0x8000_8000),
            (0x28, 0x8000_9000),
        ] {
            transaction
                .publish_resident_rom_image(rom_start, vram_start, 4)
                .unwrap();
        }
        let validated = transaction.commit().unwrap();
        let thread_id = 0xb007;
        let guest_thread_handle = 0x8000_0280;

        boot_thread0_validated_catalog_generation_program_v1(
            validated,
            install,
            test_boot_context(GuestPc::new(0x8000_7000)),
            thread_id,
            10,
        )
        .unwrap();
        crate::set_guest_running_thread_global(0x8000_8000);
        with_host(|host| {
            host.thread_handle_vrams
                .insert(thread_id, guest_thread_handle);
        });

        let mut steps = 0;
        while !crate::is_thread_dead(thread_id) {
            assert!(
                crate::run_one_step(),
                "validated scheduler-mirror thread stalled before returning"
            );
            steps += 1;
            assert!(
                steps < 4,
                "validated scheduler-mirror thread did not return"
            );
        }

        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        assert!(!rdram.is_null() && rdram_len > 0x8004);
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        assert_eq!(
            unsafe { storage.read_u32(RdramAddr::from_offset(0x8000)) },
            guest_thread_handle
        );

        let evidence = catalog_generation_install_evidence_snapshot().unwrap();
        assert!(evidence.pending_physical_writes.is_empty());
        let journal = evidence.mutation_journal.unwrap();
        assert_eq!(journal.pending_attributed_writes, 0);
        assert!(journal.open_host_transactions.is_empty());
        assert_eq!(journal.entries.len(), 2);
        let mirror = &journal.entries[1];
        assert_eq!(
            mirror.declared_writes,
            [AttributedExecutableWriteEvidenceV1 {
                channel: WriterChannel::HostAbi,
                physical_start: 0x8000,
                physical_end: 0x8004,
            }]
        );
        assert_eq!(
            mirror.changed_ranges,
            [
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x8000,
                    physical_end: 0x8001,
                },
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x8002,
                    physical_end: 0x8004,
                },
            ]
        );
        assert_ne!(mirror.before_sha256, mirror.after_sha256);
        assert!(mirror.invalidated_generations.is_empty());
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn validated_dynamic_boot_retains_input_provenance_without_static_authority() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom(rom.clone());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let bootstrap = validated.receipt().evidence().clone();

        boot_thread0_validated_catalog_generation_program_with_dynamic_mapped_v1(
            validated,
            install,
            test_boot_context(GuestPc::new(0x8000_7000)),
            0xb008,
            10,
        )
        .unwrap();

        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.rom_sha256, Some(bootstrap.rom_sha256));
        assert_eq!(
            telemetry.bootstrap_receipt_sha256,
            Some(bootstrap.receipt_sha256)
        );
        assert!(telemetry.mutation_journal_root_sha256.is_some());
        assert!(telemetry.aggregates.is_empty());
        assert!(take_validated_bootstrap_writer_channel_receipt_v1().is_none());
        assert_eq!(
            begin_cpu_writer_runtime_trace_epoch_v1().unwrap_err(),
            CpuWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert!(std::panic::catch_unwind(recompiled_program_evidence_snapshot).is_err());
        assert_eq!(canonical_block_charged_instructions_v1(), Some(0));

        crate::run_to_idle();
        assert!(crate::is_thread_dead(0xb008));
        assert_eq!(canonical_block_charged_instructions_v1(), Some(1));
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
    }

    #[test]
    fn validated_boot_rejects_a_receipt_from_another_catalog_before_installing_memory() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let expected_word = 0x2402_0001;
        let receipt_install = bootstrap_test_install(expected_word);
        let different_install = bootstrap_test_install(0x2402_0002);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom(rom.clone());
        let mut transaction = receipt_install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();

        assert!(matches!(
            boot_thread0_validated_catalog_generation_program_v1(
                transaction.commit().unwrap(),
                different_install,
                test_boot_context(GuestPc::new(0x8000_7000)),
                0xb007,
                10,
            ),
            Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
                field: "resolver_install_sha256"
            })
        ));
        with_host(|host| {
            assert!(host.owned_runtime_rdram.is_none());
            assert!(host.runtime_rdram.is_null());
            assert!(host.canonical_recompiled_program.is_none());
        });
    }

    #[test]
    fn validated_boot_rejects_a_different_installed_rom_before_installing_memory() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut receipt_rom = vec![0; 0x40];
        receipt_rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(
                &receipt_rom,
                bootstrap_test_rdram_len(),
                fn64_runtime::TvType::Ntsc,
            )
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let mut installed_rom = receipt_rom;
        installed_rom[0] = 1;
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom(installed_rom);

        assert_eq!(
            boot_thread0_validated_catalog_generation_program_v1(
                validated,
                install,
                test_boot_context(GuestPc::new(0x8000_7000)),
                0xb007,
                10,
            ),
            Err(BootstrapImportErrorV1::InstalledRomMismatch)
        );
        with_host(|host| {
            assert!(host.owned_runtime_rdram.is_none());
            assert!(host.runtime_rdram.is_null());
            assert!(host.canonical_recompiled_program.is_none());
        });
    }

    #[test]
    #[should_panic(
        expected = "canonical executable backing ends at physical RDRAM 0x00007004, beyond the installed 0x100-byte allocation"
    )]
    fn canonical_install_rejects_an_allocation_shorter_than_its_static_backing() {
        set_catalog_generation_program(bootstrap_test_install(0x2402_0001), 0x100);
    }

    #[test]
    fn catalog_resolver_install_captures_pointer_free_canonical_evidence() {
        let bank = BankId::new(0xca71);
        let program = install_test_program(bank, 0x11);
        let program_identity = program.identity();
        let build_receipt = program.build_receipt();
        let hosts = HostFunctionCatalogV1::new(vec![
            (0x8000_9000, alternate_install_test_host),
            (0x8000_8000, install_test_host),
            (INSTALL_PC.get(), install_test_host),
        ])
        .unwrap();
        let dispatch = ProgramArtifactIdentity::new([0xd1; 32]);
        let install = CatalogResolverInstallV1::new(program, hosts, dispatch);

        assert_eq!(
            install.evidence(),
            &CatalogResolverInstallEvidenceV1 {
                schema: CATALOG_RESOLVER_INSTALL_SCHEMA_V2.to_string(),
                program_identity,
                entry: ExecutionKey::new(bank, INSTALL_PC),
                instruction_budget: 2,
                host_target_pcs: vec![INSTALL_PC.get(), 0x8000_8000, 0x8000_9000],
                abi_host_catalog: None,
                dispatch_artifact_identity: dispatch,
                build_receipt,
            }
        );
        assert!(!install.has_abi_host_catalog_authority());
        assert_eq!(
            install.resolve_entry(INSTALL_PC).unwrap(),
            ExecutionKey::new(bank, INSTALL_PC)
        );
        let second_word = GuestPc::new(INSTALL_PC.get() + 4);
        assert_eq!(
            install.resolve_transfer(bank, second_word).unwrap(),
            ExecutionKey::new(bank, second_word)
        );
        let CatalogCallResolutionV1::Host(resolved_host) =
            install.resolve_call(bank, INSTALL_PC).unwrap()
        else {
            panic!("host catalog must precede an overlapping guest target");
        };
        assert!(std::ptr::fn_addr_eq(
            resolved_host,
            install_test_host as RecompFunc
        ));
        assert!(matches!(
            install.resolve_call(bank, second_word),
            Ok(CatalogCallResolutionV1::Guest(key))
                if key == ExecutionKey::new(bank, second_word)
        ));
        assert!(std::ptr::fn_addr_eq(
            install.resolve_host(0x8000_8000).unwrap(),
            install_test_host as RecompFunc
        ));
        assert!(install.resolve_host(0x8000_8004).is_none());
    }

    #[test]
    fn abi_issued_host_catalog_selects_callables_and_effects_privately() {
        let authority = issue_abi_host_function_catalog_v1(vec![
            AbiHostShimBindingV1 {
                target_pc: 0x8000_9000,
                shim: AbiHostShimV1::OsRecvMesg,
            },
            AbiHostShimBindingV1 {
                target_pc: 0x8000_8000,
                shim: AbiHostShimV1::OsSiDeviceBusy,
            },
        ])
        .unwrap();
        assert!(authority.has_valid_evidence_hash());
        assert_eq!(
            authority.evidence().bindings,
            vec![
                AbiHostShimBindingEvidenceV1 {
                    target_pc: 0x8000_8000,
                    shim: AbiHostShimV1::OsSiDeviceBusy,
                    writer_effects: vec![WriterChannel::HostAbi],
                },
                AbiHostShimBindingEvidenceV1 {
                    target_pc: 0x8000_9000,
                    shim: AbiHostShimV1::OsRecvMesg,
                    writer_effects: vec![
                        WriterChannel::CpuInstructionStore,
                        WriterChannel::PiDma,
                        WriterChannel::SiDma,
                        WriterChannel::SpDma,
                        WriterChannel::RspExecutionOrHleWriteback,
                        WriterChannel::RdpRenderer,
                        WriterChannel::HostAbi,
                    ],
                },
            ]
        );

        let install = CatalogResolverInstallV1::new_with_abi_host_catalog(
            install_test_program(BankId::new(0xca74), 0x44),
            authority,
            ProgramArtifactIdentity::new([0xd4; 32]),
        );
        assert!(install.has_abi_host_catalog_authority());
        assert!(std::ptr::fn_addr_eq(
            install.resolve_host(0x8000_8000).unwrap(),
            os_si_device_busy as RecompFunc,
        ));
        assert!(std::ptr::fn_addr_eq(
            install.resolve_host(0x8000_9000).unwrap(),
            os_recv_mesg as RecompFunc,
        ));
    }

    #[test]
    fn abi_issued_host_catalog_rejects_invalid_target_geometry() {
        assert!(matches!(
            issue_abi_host_function_catalog_v1(vec![AbiHostShimBindingV1 {
                target_pc: 0x8000_8002,
                shim: AbiHostShimV1::OsRecvMesg,
            }]),
            Err(AbiHostFunctionCatalogErrorV1::MisalignedTarget {
                target: 0x8000_8002
            })
        ));
        assert!(matches!(
            issue_abi_host_function_catalog_v1(vec![
                AbiHostShimBindingV1 {
                    target_pc: 0x8000_8000,
                    shim: AbiHostShimV1::OsRecvMesg,
                },
                AbiHostShimBindingV1 {
                    target_pc: 0x8000_8000,
                    shim: AbiHostShimV1::OsSiDeviceBusy,
                },
            ]),
            Err(AbiHostFunctionCatalogErrorV1::DuplicateTarget {
                target: 0x8000_8000
            })
        ));
    }

    #[test]
    fn abi_host_semantic_receipt_changes_resolver_and_writer_model_identity() {
        let bank = BankId::new(0xca75);
        let dispatch = ProgramArtifactIdentity::new([0xd5; 32]);
        let arbitrary = CatalogResolverInstallV1::new(
            install_test_program(bank, 0x55),
            HostFunctionCatalogV1::new(vec![(0x8000_8000, os_si_device_busy)]).unwrap(),
            dispatch,
        );
        let authority = issue_abi_host_function_catalog_v1(vec![AbiHostShimBindingV1 {
            target_pc: 0x8000_8000,
            shim: AbiHostShimV1::OsSiDeviceBusy,
        }])
        .unwrap();
        let authoritative = CatalogResolverInstallV1::new_with_abi_host_catalog(
            install_test_program(bank, 0x55),
            authority,
            dispatch,
        );
        assert_ne!(
            resolver_install_definition_sha256(&arbitrary),
            resolver_install_definition_sha256(&authoritative),
        );
        assert_ne!(
            canonical_writer_program_model_sha256(&arbitrary, None, &[]),
            canonical_writer_program_model_sha256(&authoritative, None, &[]),
        );
    }

    #[test]
    fn catalog_resolver_install_exposes_only_validated_execution_controls() {
        let first = BankId::new(0xca72);
        let second = BankId::new(0xca73);
        let hosts = HostFunctionCatalogV1::new(Vec::new()).unwrap();
        let mut install = CatalogResolverInstallV1::new(
            install_test_program(first, 0x22),
            hosts,
            ProgramArtifactIdentity::new([0xd2; 32]),
        );
        let first_identity = install.evidence().program_identity;

        assert_eq!(install.entry(), ExecutionKey::new(first, INSTALL_PC));
        install.set_budget(InstructionBudget::new(7).unwrap());
        assert_eq!(install.budget().get(), 7);
        assert_eq!(install.evidence().instruction_budget, 7);
        assert!(install
            .set_entry(ExecutionKey::new(first, GuestPc::new(INSTALL_PC.get() + 8)))
            .is_err());
        assert_eq!(install.entry(), ExecutionKey::new(first, INSTALL_PC));
        let second_word = ExecutionKey::new(first, GuestPc::new(INSTALL_PC.get() + 4));
        install.set_entry(second_word).unwrap();
        assert_eq!(install.evidence().entry, second_word);
        install
            .set_entry(ExecutionKey::new(first, INSTALL_PC))
            .unwrap();

        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();
        assert_eq!(install.run(&mut ctx, &mut mem).instructions, 1);
        assert_eq!(
            install
                .dispatch_exposing_exceptions_at(install.entry(), &mut ctx, &mut mem)
                .unwrap()
                .exit,
            BlockExit::Yield(install.entry())
        );
        assert_eq!(
            install.program_evidence().identity,
            install.evidence().program_identity
        );
        assert_eq!(install.copy_execution_destinations().len(), 2);

        install.replace_program(install_test_program(second, 0x33));
        assert_eq!(install.entry(), ExecutionKey::new(second, INSTALL_PC));
        assert_eq!(install.budget().get(), 2);
        assert_ne!(install.evidence().program_identity, first_identity);
        assert!(install.evidence().host_target_pcs.is_empty());
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_exact_static_key_withhold_is_one_shot_and_restores_static_budget() {
        let bank = BankId::new(0xca7b);
        let selected = ExecutionKey::new(bank, INSTALL_PC);
        let neighbor = ExecutionKey::new(bank, GuestPc::new(INSTALL_PC.get() + 4));
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, INSTALL_PC, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    exact_withhold_normal_budget_runner,
                    ProgramArtifactIdentity::new([0x7b; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(program, selected, InstructionBudget::new(8).unwrap())
                .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xdb; 32]),
        );
        let identity = install.evidence().program_identity;
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution_with_exact_static_key_withheld(selected);
        let mut storage = vec![0; 0x8000];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(selected),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();

        assert_eq!(run.exit, BlockExit::Yield(neighbor));
        assert_eq!(run.instructions, 2);
        assert_eq!(live.dynamic_withheld_static_key.get(), None);
        assert_eq!(live.install.evidence().program_identity, identity);
        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.aggregates.len(), 1);
        assert_eq!(telemetry.aggregates[0].charged_instructions, 1);
        assert_eq!(
            telemetry.aggregates[0].attempted_entries,
            vec![DynamicMappedEntryCountV1 {
                attempted_entry: selected,
                activations: 1,
                charged_instructions: 1,
                unsupported_exits: 0,
            }]
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_exact_static_key_withhold_rejects_non_entry_member() {
        let program = install_test_program(INSTALL_BANK, 0x7a);
        let install = CatalogResolverInstallV1::new(
            program,
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xda; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        let selected = ExecutionKey::new(INSTALL_BANK, GuestPc::new(INSTALL_PC.get() + 4));

        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            live.enable_dynamic_mapped_execution_with_exact_static_key_withheld(selected);
        }))
        .expect_err("a non-entry static key was accepted for one-shot withholding");
        let failure = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(failure.contains("must select the canonical catalog entry"));
        assert!(live.dynamic_units.borrow().is_none());
        assert_eq!(live.dynamic_withheld_static_key.get(), None);
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn dynamic_attempted_alias_with_zero_charge_cannot_borrow_aggregate_work() {
        let live = set_catalog_block_program(
            CatalogResolverInstallV1::new(
                install_test_program(INSTALL_BANK, 0x79),
                HostFunctionCatalogV1::new(Vec::new()).unwrap(),
                ProgramArtifactIdentity::new([0xd9; 32]),
            ),
            0x8000,
        );
        live.enable_dynamic_mapped_execution();
        let first = ExecutionKey::new(INSTALL_BANK, INSTALL_PC);
        let alias = ExecutionKey::new(INSTALL_BANK, GuestPc::new(0xa000_7000));
        let mut storage = vec![0; 0x8000];
        put_physical_word(&mut storage, 0x7000, 0x1000_0001);
        put_physical_word(&mut storage, 0x7004, 0);
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();

        let positive = live
            .dynamic_units
            .borrow_mut()
            .as_mut()
            .unwrap()
            .activate_and_run(
                first,
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |bank| live.reserves_bank(bank),
            )
            .unwrap();
        assert_eq!(positive.run.instructions, 2);
        live.record_dynamic_execution(first, &positive);

        let zero = live
            .dynamic_units
            .borrow_mut()
            .as_mut()
            .unwrap()
            .activate_and_run(
                alias,
                InstructionBudget::new(1).unwrap(),
                &mut ctx,
                &mut mem,
                |bank| live.reserves_bank(bank),
            )
            .unwrap();
        assert_eq!(zero.identity, positive.identity);
        assert_eq!(zero.run.instructions, 0);
        live.record_dynamic_execution(alias, &zero);

        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        let [aggregate] = telemetry.aggregates.as_slice() else {
            panic!("expected one shared dynamic identity: {telemetry:?}");
        };
        assert_eq!(aggregate.charged_instructions, 2);
        let first_count = aggregate
            .attempted_entries
            .iter()
            .find(|entry| entry.attempted_entry == first)
            .unwrap();
        let alias_count = aggregate
            .attempted_entries
            .iter()
            .find(|entry| entry.attempted_entry == alias)
            .unwrap();
        assert_eq!(first_count.charged_instructions, 2);
        assert_eq!(alias_count.charged_instructions, 0);
        assert_eq!(alias_count.activations, 1);
        assert_eq!(alias_count.unsupported_exits, 0);
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_exact_static_key_withhold_preserves_branch_delay_budget() {
        let bank = BankId::new(0xca7a);
        let selected = ExecutionKey::new(bank, INSTALL_PC);
        let target = ExecutionKey::new(bank, GuestPc::new(INSTALL_PC.get() + 8));
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, INSTALL_PC, vec![0, 0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    install_test_runner,
                    ProgramArtifactIdentity::new([0x7a; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(program, selected, InstructionBudget::new(3).unwrap())
                .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xda; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution_with_exact_static_key_withheld(selected);
        let mut storage = vec![0; 0x8000];
        put_physical_word(&mut storage, INSTALL_PC.get() & 0x1fff_ffff, 0x1000_0001);
        put_physical_word(&mut storage, (INSTALL_PC.get() & 0x1fff_ffff) + 4, 0);
        let mut mem = Rdram::new(&mut storage);

        let error = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(selected),
            InstructionBudget::new(1).unwrap(),
            &mut RsContext::new(),
            &mut mem,
        )
        .expect_err("one instruction split a withheld branch/delay pair");
        assert!(error.contains("indivisible instruction unit"));
        assert_eq!(live.dynamic_withheld_static_key.get(), Some(selected));
        assert!(copy_dynamic_mapped_execution_telemetry_v1()
            .aggregates
            .is_empty());

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(selected),
            InstructionBudget::new(3).unwrap(),
            &mut RsContext::new(),
            &mut mem,
        )
        .unwrap();
        assert_eq!(run.exit, BlockExit::Yield(target));
        assert_eq!(run.instructions, 3);
        assert_eq!(live.dynamic_withheld_static_key.get(), None);
        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.aggregates.len(), 1);
        assert_eq!(telemetry.aggregates[0].charged_instructions, 2);
        assert_eq!(
            telemetry.aggregates[0].attempted_entries[0].attempted_entry,
            selected
        );
        assert_eq!(
            telemetry.aggregates[0].attempted_entries[0].charged_instructions,
            2
        );
        assert_eq!(
            telemetry.aggregates[0].attempted_entries[0].unsupported_exits,
            0
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_budget_static_miss_dynamic_static_no_replay() {
        let bank = BankId::new(0xca7d);
        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let static_resume = GuestPc::new(INSTALL_PC.get() + 0x20);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::from_spans(
                    bank,
                    vec![
                        CodeSpan::new(bank, INSTALL_PC, vec![0]).unwrap(),
                        CodeSpan::new(bank, static_resume, vec![0]).unwrap(),
                    ],
                )
                .unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    unified_transition_test_runner,
                    ProgramArtifactIdentity::new([0x7d; 32]),
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
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xdd; 32]),
        );
        let static_identity = install.evidence().program_identity;
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();

        let mut storage = vec![0; 0x8000];
        let jump = 0x0800_0000 | ((static_resume.get() >> 2) & 0x03ff_ffff);
        put_physical_word(&mut storage, dynamic_pc.get() & 0x1fff_ffff, jump);
        put_physical_word(&mut storage, (dynamic_pc.get() & 0x1fff_ffff) + 4, 0);
        let mut mem = Rdram::new(&mut storage);

        let mut checkpoint_ctx = RsContext::new();
        let checkpoint = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(bank, INSTALL_PC)),
            InstructionBudget::new(2).unwrap(),
            &mut checkpoint_ctx,
            &mut mem,
        )
        .expect("prior static work must checkpoint before a final dynamic branch/delay pair");
        assert_eq!(
            checkpoint.exit,
            BlockExit::Checkpoint(ExecutionKey::new(bank, dynamic_pc))
        );
        assert_eq!(checkpoint.instructions, 1);
        assert_eq!(checkpoint.blocks, 1);
        assert_eq!(checkpoint_ctx.r_u32(2), 1);
        assert_eq!(
            live.dynamic_units
                .borrow()
                .as_ref()
                .expect("dynamic catalog remains installed")
                .admitted_len(),
            1,
            "classifying the indivisible unit admits its exact fetched identity"
        );
        assert!(
            copy_dynamic_mapped_execution_telemetry_v1()
                .aggregates
                .is_empty(),
            "a rejected indivisible unit must not publish execution telemetry"
        );

        let mut ctx = RsContext::new();
        set_canonical_block_instruction_limit_v1(Some(4));
        assert_eq!(live.next_dispatch_budget().get(), 4);

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(bank, INSTALL_PC)),
            live.next_dispatch_budget(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();

        assert_eq!(
            run.exit,
            BlockExit::Yield(ExecutionKey::new(bank, static_resume))
        );
        assert_eq!(run.instructions, 4);
        live.charge_canonical_instructions(run.instructions);
        assert_eq!(live.canonical_charged_instructions.get(), 4);
        let split = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = live.next_dispatch_budget();
        }))
        .expect_err("dispatch may not continue past the exact limit");
        let split = split
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| split.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(split.contains("limit 4 was already reached"));
        assert_eq!(ctx.r_u32(2), 1, "static source replayed after its miss");
        assert_eq!(
            ctx.r_u32(3),
            1,
            "one-instruction static continuation did not reach the exact ceiling"
        );
        assert_eq!(live.install.evidence().program_identity, static_identity);
        assert_eq!(
            live.dynamic_units
                .borrow()
                .as_ref()
                .expect("dynamic catalog remains installed")
                .admitted_len(),
            1
        );
        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.aggregates.len(), 1);
        assert_eq!(telemetry.dropped_identity_activations, 0);
        assert_eq!(telemetry.dropped_attempted_entry_activations, 0);
        assert_eq!(telemetry.aggregates[0].activations, 1);
        assert_eq!(telemetry.aggregates[0].charged_instructions, 2);
        assert_eq!(telemetry.aggregates[0].unsupported_exits, 0);
        assert_eq!(
            telemetry.aggregates[0].attempted_entries,
            vec![DynamicMappedEntryCountV1 {
                attempted_entry: ExecutionKey::new(bank, dynamic_pc),
                activations: 1,
                charged_instructions: 2,
                unsupported_exits: 0,
            }]
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_call_host_precedes_dynamic() {
        let bank = BankId::new(0xca7e);
        let host_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let resume = ExecutionKey::new(bank, GuestPc::new(INSTALL_PC.get() + 0x20));
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::from_spans(
                    bank,
                    vec![
                        CodeSpan::new(bank, INSTALL_PC, vec![0]).unwrap(),
                        CodeSpan::new(bank, resume.pc, vec![0]).unwrap(),
                    ],
                )
                .unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    unified_host_precedence_runner,
                    ProgramArtifactIdentity::new([0x7e; 32]),
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
            HostFunctionCatalogV1::new(vec![(host_pc.get(), install_test_host)]).unwrap(),
            ProgramArtifactIdentity::new([0xde; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();
        let mut storage = vec![0; 0x8000];
        put_physical_word(&mut storage, host_pc.get() & 0x1fff_ffff, 0x2402_0063);
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(bank, INSTALL_PC)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();

        assert_eq!(
            run.exit,
            BlockExit::HostCall {
                vram: host_pc,
                resume,
            }
        );
        assert_eq!(run.instructions, 1);
        assert_eq!(ctx.r_u32(2), 1);
        assert_eq!(
            live.dynamic_units
                .borrow()
                .as_ref()
                .expect("dynamic catalog remains installed")
                .admitted_len(),
            0,
            "an exact host binding must win before dynamic admission"
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_precompiled_activation_precedes_dynamic() {
        let generation_word = 0x2402_0001;
        let generation_pc = GuestPc::new(0x8000_a000);
        let generation_bank = BankId::new(0xb00a);
        let live = set_catalog_generation_program(
            bootstrap_test_install_with_generation(0, generation_word),
            0xb000,
        );
        live.enable_dynamic_mapped_execution();
        let mut storage = vec![0; 0xb000];
        put_physical_word(&mut storage, 0xa000, generation_word);
        let mem = Rdram::new(&mut storage);

        let target =
            resolve_unified_catalog_target(&live, INSTALL_BANK, generation_pc, &mem).unwrap();

        assert_eq!(
            target,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(generation_bank, generation_pc))
        );
        assert_eq!(
            live.dynamic_units
                .borrow()
                .as_ref()
                .expect("dynamic catalog remains installed")
                .admitted_len(),
            0,
            "a digest-matched precompiled generation must win before dynamic admission"
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_dynamic_fetch_fault_preserves_prior_work() {
        let bank = BankId::new(0xca7f);
        let target_pc = GuestPc::new(0x0040_0000);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, INSTALL_PC, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    unified_tlb_fault_runner,
                    ProgramArtifactIdentity::new([0x7f; 32]),
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
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xdf; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();
        let mut storage = vec![0; 0x8000];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();
        ctx.initialize_invalid_tlb_entries();

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(bank, INSTALL_PC)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();

        assert_eq!(run.instructions, 2, "source work plus faulting fetch");
        assert_eq!(ctx.r_u32(2), 1, "static source must not replay");
        assert!(matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                at: ExecutionKey { pc, .. },
                kind: CpuFaultKind::Exception {
                    exception: CpuException::TlbRefillLoad,
                    epc,
                    branch_delay: false,
                    bad_vaddr: Some(0x0040_0000),
                    ..
                },
            }) if pc == target_pc && epc == target_pc
        ));
        assert_eq!(
            live.dynamic_units
                .borrow()
                .as_ref()
                .expect("dynamic catalog remains installed")
                .admitted_len(),
            0
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_delay_store_refetches_dynamic_target() {
        let bank = BankId::new(0xca80);
        let branch_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let target_pc = GuestPc::new(branch_pc.get() + 0x0c);
        let static_resume = GuestPc::new(INSTALL_PC.get() + 0x40);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::from_spans(
                    bank,
                    vec![
                        CodeSpan::new(bank, INSTALL_PC, vec![0]).unwrap(),
                        CodeSpan::new(bank, static_resume, vec![0]).unwrap(),
                    ],
                )
                .unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    unified_dynamic_writer_runner,
                    ProgramArtifactIdentity::new([0x80; 32]),
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
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xe0; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();
        let mut storage = vec![0; 0x8000];
        put_physical_word(&mut storage, branch_pc.get() & 0x1fff_ffff, 0x1000_0002);
        put_physical_word(
            &mut storage,
            (branch_pc.get() & 0x1fff_ffff) + 4,
            0xac88_0000,
        );
        put_physical_word(&mut storage, target_pc.get() & 0x1fff_ffff, 0x2442_0001);
        put_physical_word(&mut storage, (target_pc.get() & 0x1fff_ffff) + 4, 0);
        let replacement_jump = 0x0800_0000 | ((static_resume.get() >> 2) & 0x03ff_ffff);
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();
        ctx.set_r(4, 0xffff_ffff_0000_0000 | u64::from(target_pc.get()));
        ctx.set_r(8, u64::from(replacement_jump));

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(bank, INSTALL_PC)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();

        assert_eq!(
            run.exit,
            BlockExit::Yield(ExecutionKey::new(bank, static_resume))
        );
        assert_eq!(run.instructions, 6);
        assert_eq!(ctx.r_u32(2), 0, "stale target instruction executed");
        assert_eq!(ctx.r_u32(3), 1);
        assert_eq!(
            mem.load_w(0xffff_ffff_0000_0000 | u64::from(target_pc.get())) as u32,
            replacement_jump
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_executable_write_publishes_before_continuation() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let bank = BankId::new(0xca82);
        let entry = ExecutionKey::new(bank, INSTALL_PC);
        let resume = ExecutionKey::new(bank, GuestPc::new(INSTALL_PC.get() + 4));
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, INSTALL_PC, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    unified_executable_write_boundary_runner,
                    ProgramArtifactIdentity::new([0x82; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(program, entry, InstructionBudget::new(8).unwrap()).unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xe2; 32]),
        );
        let mut storage = vec![0; 0x8000];
        let thread_id = 0xca82;

        // SAFETY: `storage` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_program_with_dynamic_mapped_v1(
                storage.as_mut_ptr(),
                storage.len(),
                install,
                test_boot_context(INSTALL_PC),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(write)] = publications.as_slice() else {
            panic!("expected executable-write publication: {publications:?}");
        };
        assert_eq!(
            write.pending_exit,
            BlockExit::ExecutableWrite {
                source_bank: bank,
                resume,
            }
        );
        assert_eq!(write.charged_instructions, 1);
        assert_eq!(
            write.cpu.gprs[2], 0,
            "continuation crossed the write boundary"
        );
        assert!(!crate::is_thread_dead(thread_id));

        assert!(crate::run_one_step());
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(continuation)] = publications.as_slice() else {
            panic!("expected resumed continuation publication: {publications:?}");
        };
        assert_eq!(continuation.pending_exit, BlockExit::ThreadReturn);
        assert_eq!(continuation.charged_instructions, 1);
        assert_eq!(continuation.cpu.gprs[2], 1);
        crate::run_to_idle();
        assert!(crate::is_thread_dead(thread_id));
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_unsupported_dynamic_word_is_loud_with_prior_count() {
        let bank = BankId::new(0xca81);
        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let static_resume = GuestPc::new(INSTALL_PC.get() + 0x20);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::from_spans(
                    bank,
                    vec![
                        CodeSpan::new(bank, INSTALL_PC, vec![0]).unwrap(),
                        CodeSpan::new(bank, static_resume, vec![0]).unwrap(),
                    ],
                )
                .unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    unified_transition_test_runner,
                    ProgramArtifactIdentity::new([0x81; 32]),
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
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xe1; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();
        let mut storage = vec![0; 0x8000];
        put_physical_word(&mut storage, dynamic_pc.get() & 0x1fff_ffff, 0x4800_0000); // mfc2 zero,cop2r0
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(bank, INSTALL_PC)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();

        assert_eq!(run.instructions, 1, "only the static source retired");
        assert_eq!(ctx.r_u32(2), 1, "static source replayed after its miss");
        assert!(matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                at: ExecutionKey { pc, .. },
                kind: CpuFaultKind::UnsupportedInstruction { word: 0x4800_0000 },
            }) if pc == dynamic_pc
        ));
        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.aggregates.len(), 1);
        assert_eq!(telemetry.dropped_identity_unsupported_exits, 0);
        assert_eq!(telemetry.aggregates[0].charged_instructions, 0);
        assert_eq!(telemetry.aggregates[0].unsupported_exits, 1);
        assert_eq!(telemetry.aggregates[0].attempted_entries.len(), 1);
        assert_eq!(
            telemetry.aggregates[0].attempted_entries[0],
            DynamicMappedEntryCountV1 {
                attempted_entry: ExecutionKey::new(bank, dynamic_pc),
                activations: 1,
                charged_instructions: 0,
                unsupported_exits: 1,
            }
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_entry_ambiguity_cannot_fall_back_or_mint_static_evidence() {
        let first = BankId::new(0xca82);
        let second = BankId::new(0xca83);
        let mut program = BlockProgram::new();
        for (bank, artifact) in [(first, 0x82), (second, 0x83)] {
            program
                .register(
                    CodeBank::new(bank, INSTALL_PC, vec![0]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        install_test_runner,
                        ProgramArtifactIdentity::new([artifact; 32]),
                    ),
                )
                .unwrap();
        }
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(first, INSTALL_PC),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xe2; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();
        let mut storage = vec![0; 0x8000];
        let mem = Rdram::new(&mut storage);

        let error = resolve_unified_catalog_entry(&live, INSTALL_PC, &mem).unwrap_err();
        assert!(
            error.contains("ambiguous"),
            "bankless entry ambiguity was hidden by dynamic fallback: {error}"
        );
        let evidence = std::panic::catch_unwind(recompiled_program_evidence_snapshot);
        assert!(
            evidence.is_err(),
            "dynamic execution exposed an incomplete static program-evidence snapshot"
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_dynamic_install_rejects_all_static_writer_authority_paths() {
        let bank = BankId::new(0xca84);
        let install = CatalogResolverInstallV1::new(
            install_test_program(bank, 0x84),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xe4; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();

        assert_eq!(
            live.mint_bootstrap_writer_completion(&[]).unwrap_err(),
            BootstrapWriterChannelCompletionErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.begin_cpu_writer_runtime_trace_epoch().unwrap_err(),
            CpuWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.begin_host_abi_writer_runtime_trace_epoch()
                .unwrap_err(),
            HostAbiWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.begin_rsp_writer_runtime_trace_epoch().unwrap_err(),
            RspWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.begin_rdp_renderer_writer_runtime_trace_epoch()
                .unwrap_err(),
            RdpRendererWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.begin_pi_writer_runtime_trace_epoch(false, false, false)
                .unwrap_err(),
            PiWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.take_si_writer_runtime_state(&[], false, &[], false, false)
                .unwrap_err(),
            SiWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.begin_sp_writer_runtime_trace_epoch().unwrap_err(),
            SpWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
    }

    #[test]
    fn catalog_resolver_install_preserves_fail_closed_static_resolution() {
        let first = BankId::new(0xca74);
        let second = BankId::new(0xca75);
        let unique = BankId::new(0xca76);
        let unique_pc = GuestPc::new(0x8000_a000);
        let mut program = BlockProgram::new();
        for (bank, pc, artifact_byte) in [
            (second, INSTALL_PC, 0x42),
            (first, INSTALL_PC, 0x41),
            (unique, unique_pc, 0x43),
        ] {
            program
                .register(
                    CodeBank::new(bank, pc, vec![0]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        install_test_runner,
                        ProgramArtifactIdentity::new([artifact_byte; 32]),
                    ),
                )
                .unwrap();
        }
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(first, INSTALL_PC),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xd4; 32]),
        );
        let evidence = install.evidence().clone();

        assert!(matches!(
            install.resolve_entry(INSTALL_PC),
            Err(CpuFault {
                kind: CpuFaultKind::AmbiguousPc {
                    first_candidate,
                    second_candidate,
                    candidate_count: 2,
                },
                ..
            }) if first_candidate == first && second_candidate == second
        ));
        assert_eq!(
            install.resolve_transfer(second, INSTALL_PC).unwrap(),
            ExecutionKey::new(second, INSTALL_PC)
        );
        assert_eq!(
            install.resolve_transfer(first, unique_pc).unwrap(),
            ExecutionKey::new(unique, unique_pc)
        );

        let sparse_hole = GuestPc::new(INSTALL_PC.get() + 4);
        assert!(matches!(
            install.resolve_transfer(first, sparse_hole),
            Err(CpuFault {
                kind: CpuFaultKind::UnmappedPc { .. },
                ..
            })
        ));
        let misaligned = GuestPc::new(INSTALL_PC.get() + 2);
        assert!(matches!(
            install.resolve_entry(misaligned),
            Err(CpuFault {
                kind: CpuFaultKind::Exception {
                    exception: CpuException::AddressErrorLoad,
                    ..
                },
                ..
            })
        ));

        let previous = fn64_recomp_rs::set_host_lookup(Some(install_test_legacy_host_lookup));
        assert!(matches!(
            install.resolve_call(first, sparse_hole),
            Err(CpuFault {
                kind: CpuFaultKind::UnmappedPc { .. },
                ..
            })
        ));
        fn64_recomp_rs::set_host_lookup(previous);
        assert_eq!(install.evidence(), &evidence);
    }

    #[test]
    fn only_canonical_install_populates_catalog_evidence_and_legacy_clears_it() {
        let bank = BankId::new(0xca77);
        let install = CatalogResolverInstallV1::new(
            install_test_program(bank, 0x61),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xd5; 32]),
        );
        let expected = install.evidence().clone();
        set_catalog_block_program(install, 0x7008);
        assert_eq!(
            catalog_resolver_install_evidence_snapshot(),
            Some(expected.clone())
        );
        assert!(matches!(
            recompiled_program_evidence_snapshot(),
            Some(RecompiledProgramEvidenceSnapshot::Block {
                dispatch_artifact_identity,
                instruction_budget: 2,
                ref executable_regions,
                ref pending_executable_writes,
                ..
            }) if dispatch_artifact_identity == expected.dispatch_artifact_identity
                && executable_regions.is_empty()
                && pending_executable_writes.is_empty()
        ));

        set_entry_lookup(install_test_function_lookup, 0x100);
        assert_eq!(catalog_resolver_install_evidence_snapshot(), None);

        let second = CatalogResolverInstallV1::new(
            install_test_program(bank, 0x62),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xd6; 32]),
        );
        set_catalog_block_program(second, 0x7008);
        let legacy = LiveBlockProgram {
            program: Rc::new(RefCell::new(BlockProgram::new())),
            entry_lookup: install_test_entry_lookup,
            transfer_lookup: install_test_transfer_lookup,
            budget: InstructionBudget::new(2).unwrap(),
            dispatch_artifact_identity: None,
            executable_regions: Rc::new(RefCell::new(Vec::new())),
            precompiled_generations: Rc::new(RefCell::new(None)),
        };
        set_block_program(legacy, 0x100);
        assert_eq!(catalog_resolver_install_evidence_snapshot(), None);
    }

    #[test]
    fn catalog_resolver_feature_predicate_is_only_lane_eligibility() {
        let eligible = StaticExecutionBuildReceipt {
            schema: 1,
            aot_runtime: true,
            production_aot: true,
            dev_interpreter: false,
        };
        assert!(catalog_resolver_feature_lane_eligible(eligible));
        assert!(!catalog_resolver_feature_lane_eligible(
            StaticExecutionBuildReceipt {
                production_aot: false,
                ..eligible
            }
        ));
        assert!(!catalog_resolver_feature_lane_eligible(
            StaticExecutionBuildReceipt {
                aot_runtime: false,
                ..eligible
            }
        ));
        assert!(!catalog_resolver_feature_lane_eligible(
            StaticExecutionBuildReceipt {
                dev_interpreter: true,
                ..eligible
            }
        ));
    }

    #[test]
    #[should_panic(expected = "catalog does not match the live BlockProgram")]
    fn installing_generation_catalog_rejects_a_missing_shard_bank() {
        let live = LiveBlockProgram {
            program: Rc::new(RefCell::new(BlockProgram::new())),
            entry_lookup: live_entry_lookup,
            transfer_lookup: live_transfer_lookup,
            budget: InstructionBudget::new(2).unwrap(),
            dispatch_artifact_identity: None,
            executable_regions: Rc::new(RefCell::new(Vec::new())),
            precompiled_generations: Rc::new(RefCell::new(None)),
        };
        set_block_program(live, 0x100);
        let start = GuestPc::new(0x8000_0100);
        let end = GuestPc::new(start.get() + 4);
        let bank = BankId::new(0xBAD);
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog
            .register(
                PrecompiledGeneration::new(
                    GenerationId::new(1),
                    start,
                    end,
                    start,
                    end,
                    [0; 32],
                    vec![PrecompiledShard::new(bank, start, end).unwrap()],
                )
                .unwrap(),
            )
            .unwrap();

        install_precompiled_generation_catalog(catalog);
    }

    fn test_boot_context(entry: GuestPc) -> BootContext {
        if with_host(|host| host.device_fabric.tv_type()).is_none() {
            crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        }
        if with_host(|host| host.installed_rom).is_none() {
            crate::load_rom(vec![0]);
        }
        let rom_sha256 = with_host(|host| {
            host.installed_rom
                .expect("test ROM was installed above")
                .sha256
        });
        let mut gprs = [0u64; 32];
        gprs[31] = u64::from(THREAD_RETURN_SENTINEL);
        let (hi, lo) = if entry == LIVE_ENTRY {
            gprs[20] = 0xffff_ffff_cafe_babe;
            (0x1234, 0x5678)
        } else {
            (0, 0)
        };
        let mut cp0 = [0u64; 32];
        cp0[1] = 31;
        BootContext {
            schema: BOOT_CONTEXT_SCHEMA_V1.to_string(),
            producer: "fn64-abi synthetic block test".to_string(),
            normalized_rom_sha256: Sha256Digest::from_bytes(rom_sha256),
            cic: BootCicIdentity {
                ipl3_sha256: Sha256Digest::from_bytes([0; 32]),
            },
            region: BootRegion {
                destination_code: b'E',
                tv_standard: BootTvStandard::Ntsc,
            },
            entry_pc: entry.get(),
            gprs,
            hi,
            lo,
            cp0: BootCop0Context { registers: cp0 },
        }
    }

    #[test]
    fn catalog_boot_context_is_checked_before_unified_dispatch() {
        let entry = ExecutionKey::new(INSTALL_BANK, INSTALL_PC);
        let boot_context = test_boot_context(INSTALL_PC);
        let mut ctx = RsContext::new();
        ctx.restore_boot_context(&boot_context).unwrap();
        validate_restored_catalog_boot_context(entry, &boot_context, &ctx);

        ctx.set_r32(20, 1);
        let state_failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            validate_restored_catalog_boot_context(entry, &boot_context, &ctx);
        }))
        .expect_err("a mismatched restored boot register reached unified dispatch");
        let state_failure = state_failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| state_failure.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(state_failure.contains("before first unified dispatch"));

        let entry_failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            validate_restored_catalog_boot_context(
                ExecutionKey::new(INSTALL_BANK, GuestPc::new(INSTALL_PC.get() + 4)),
                &boot_context,
                &RsContext::new(),
            );
        }))
        .expect_err("a non-BootContext entry reached first unified dispatch");
        let entry_failure = entry_failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| entry_failure.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(entry_failure.contains("dispatch entry differs"));
    }

    const LIVE_BANK: BankId = BankId::new(0xA11CE);
    const LIVE_SECOND_BANK: BankId = BankId::new(0xA11CF);
    const LIVE_ENTRY: GuestPc = GuestPc::new(0x8000_1000);
    const LIVE_NEXT: GuestPc = GuestPc::new(0x8000_1004);
    const LIVE_HOST: GuestPc = GuestPc::new(0x8000_2000);
    const ORDERED_WRITER_BANK: BankId = BankId::new(0x0ade_0001);
    const ORDERED_WRITER_ENTRY: GuestPc = GuestPc::new(0x8000_7000);
    const ORDERED_WRITER_RESUME: GuestPc = GuestPc::new(0x8000_7004);
    const ORDERED_WRITER_HOST: GuestPc = GuestPc::new(0x8000_7100);
    const ORDERED_SYNC_BANK: BankId = BankId::new(0x0ade_0002);
    const ORDERED_SYNC_ENTRY: GuestPc = GuestPc::new(0x8000_7200);
    const ORDERED_SYNC_RESUME: GuestPc = GuestPc::new(0x8000_7204);
    const ORDERED_SYNC_HOST: GuestPc = GuestPc::new(0x8000_7300);
    const CATALOG_REWRITE_ENTRY: GuestPc = GuestPc::new(0x8000_6000);
    const CATALOG_REWRITE_A: BankId = BankId::new(0xca80);
    const CATALOG_REWRITE_B: BankId = BankId::new(0xca81);
    const PREPARED_STATIC_BANK: BankId = BankId::new(0xca90);
    const PREPARED_GENERATION_BANK: BankId = BankId::new(0xca91);
    const PREPARED_STATIC_ENTRY: GuestPc = GuestPc::new(0x8000_5000);
    const PREPARED_GENERATION_ENTRY: GuestPc = GuestPc::new(0x8000_6000);
    const IRQ_BANK: BankId = BankId::new(0x1A2);
    const IRQ_ENTRY: GuestPc = GuestPc::new(0x8000_0100);
    const IRQ_RESUME: GuestPc = GuestPc::new(0x8000_0104);
    const IRQ_VECTOR: GuestPc = GuestPc::new(0x8000_0180);
    const TIMER_BANK: BankId = BankId::new(0x1A7);
    const REWRITE_OLD_BANK: BankId = BankId::new(0xC0DE_0000);
    const REWRITE_NEW_BANK: BankId = BankId::new(0xC0DE_0001);
    const REWRITE_ENTRY: GuestPc = GuestPc::new(0x8000_3000);
    const REWRITE_RESUME: GuestPc = GuestPc::new(REWRITE_ENTRY.get() + 0x24);
    const REWRITE_PHYSICAL: u32 = 0x80;
    const REWRITE_A_WORDS: [u32; 13] = [
        0x3c09_8000, // lui t1, 0x8000
        0x240c_0055, // addiu t4, zero, 0x55
        0xad2c_0020, // sw t4, 0x20(t1) -- non-executable store
        0x240d_0066, // addiu t5, zero, 0x66
        0xad2d_0024, // sw t5, 0x24(t1) -- proves ordinary stores do not split
        0x240a_0001, // addiu t2, zero, 1 -- prepare the post-store sentinel
        0x3c08_1122, // lui t0, 0x1122
        0x3508_3344, // ori t0, t0, 0x3344
        0xad28_0080, // sw t0, 0x80(t1) -- replaces this executable image
        0xad2a_0010, // generation-A post-store sentinel
        0x03e0_0008, // jr ra
        0,
        0,
    ];
    const REWRITE_B_WORDS: [u32; 13] = [
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0x240b_0002, // addiu t3, zero, 2
        0xad2b_0014, // sw t3, 0x14(t1)
        0x03e0_0008, // jr ra
        0,
    ];
    const DMA_OLD_BANK: BankId = BankId::new(0xD00D_0000);
    const DMA_NEW_BANK: BankId = BankId::new(0xD00D_0001);
    const DMA_ENTRY: GuestPc = GuestPc::new(0x8000_4000);
    const DMA_PHYSICAL: u32 = 0x100;

    thread_local! {
        static LIVE_ACTIVE_BANK: std::cell::Cell<BankId> = const { std::cell::Cell::new(LIVE_BANK) };
        static REWRITE_BUILDS: std::cell::RefCell<Vec<(u64, Vec<u8>)>> = const {
            std::cell::RefCell::new(Vec::new())
        };
        static REWRITE_B_ENTRIES: std::cell::RefCell<Vec<ExecutionKey>> = const {
            std::cell::RefCell::new(Vec::new())
        };
        static BOOT_FPCSR_OBSERVATIONS: std::cell::RefCell<Vec<u32>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }

    fn evidence_callable(_ctx: &mut RsContext, _mem: &mut Rdram<'_>) {}

    fn alternate_evidence_callable(_ctx: &mut RsContext, _mem: &mut Rdram<'_>) {}

    fn evidence_lookup(_vram: u32) -> RecompFunc {
        evidence_callable
    }

    fn alternate_evidence_lookup(_vram: u32) -> RecompFunc {
        alternate_evidence_callable
    }

    fn observe_thread0_fpcsr_boot(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
        BOOT_FPCSR_OBSERVATIONS.with(|observed| observed.borrow_mut().push(ctx.read_fcr(31)));
        os_initialize(ctx, mem);
        BOOT_FPCSR_OBSERVATIONS.with(|observed| observed.borrow_mut().push(ctx.read_fcr(31)));
    }

    fn unused_evidence_builder(
        _bytes: &[u8],
        _generation: u64,
    ) -> Result<(CodeBank, GeneratedBankRunner), String> {
        Err("evidence-only builder must not run".to_string())
    }

    fn alternate_unused_evidence_builder(
        _bytes: &[u8],
        _generation: u64,
    ) -> Result<(CodeBank, GeneratedBankRunner), String> {
        Err("alternate evidence-only builder must not run".to_string())
    }

    fn install_evidence_block_lane(budget: u32, reverse_regions: bool, alternate_builders: bool) {
        let first_bank = BankId::new(0xE100);
        let second_bank = BankId::new(0xE200);
        let first_start = GuestPc::new(0x8000_5000);
        let second_start = GuestPc::new(0x8000_6000);
        let mut program = BlockProgram::new();
        let mut first_region =
            ExecutableRegion::new(first_start, GuestPc::new(first_start.get() + 4));
        let mut second_region =
            ExecutableRegion::new(second_start, GuestPc::new(second_start.get() + 4));
        let runner_artifact = ProgramArtifactIdentity::new([0xE5; 32]);
        first_region
            .install(
                &mut program,
                CodeBank::new(first_bank, first_start, vec![0x1111_2222]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    first_bank,
                    live_test_runner,
                    runner_artifact,
                ),
            )
            .unwrap();
        second_region
            .install(
                &mut program,
                CodeBank::new(second_bank, second_start, vec![0x3333_4444]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    second_bank,
                    live_test_runner,
                    runner_artifact,
                ),
            )
            .unwrap();
        let live = LiveBlockProgram {
            program: Rc::new(RefCell::new(program)),
            entry_lookup: live_entry_lookup,
            transfer_lookup: live_transfer_lookup,
            budget: InstructionBudget::new(budget).unwrap(),
            dispatch_artifact_identity: Some(ProgramArtifactIdentity::new([0xD1; 32])),
            executable_regions: Rc::new(RefCell::new(Vec::new())),
            precompiled_generations: Rc::new(RefCell::new(None)),
        };
        set_block_program(live, 0x100);
        let first_builder = if alternate_builders {
            alternate_unused_evidence_builder
        } else {
            unused_evidence_builder
        };
        let registrations = [
            (0x20, 0x24, first_region, first_builder),
            (0x40, 0x44, second_region, unused_evidence_builder),
        ];
        if reverse_regions {
            for (start, end, region, builder) in registrations.into_iter().rev() {
                register_live_executable_region_with_artifact_identity(
                    start,
                    end,
                    region,
                    builder,
                    ProgramArtifactIdentity::new([0xB1; 32]),
                );
            }
        } else {
            for (start, end, region, builder) in registrations {
                register_live_executable_region_with_artifact_identity(
                    start,
                    end,
                    region,
                    builder,
                    ProgramArtifactIdentity::new([0xB1; 32]),
                );
            }
        }
    }

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
        with_host(|host| *host = super::super::HostState::default());
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
        with_host(|host| *host = super::super::HostState::default());
        let identity = ProgramArtifactIdentity::new([0xC3; 32]);
        set_entry_lookup_with_execution_observation(
            evidence_lookup,
            0x100,
            identity,
            fn64_recomp_rs::FUNCTION_ENTRY_OBSERVATION_SCHEMA,
        );
        with_executor(|executor| executor.set_sim_time(37));
        fn64_recomp_rs::notify_function_entry(TranslatedFunctionIdentity::new(
            0x8000_1000,
            "entry",
        ));
        with_executor(|executor| executor.set_sim_time(41));
        fn64_recomp_rs::notify_function_entry(TranslatedFunctionIdentity::new(
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

    #[test]
    fn block_lane_evidence_sorts_regions_and_excludes_builder_pointers() {
        with_host(|host| *host = super::super::HostState::default());
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
        with_host(|host| *host = super::super::HostState::default());
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
        with_host(|host| *host = super::super::HostState::default());
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
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().push((0x30, 0)));
        let _ = recompiled_program_evidence_snapshot();
    }

    #[test]
    #[should_panic(expected = "stable host-provided dispatch artifact identity")]
    fn block_lane_evidence_rejects_unidentified_dispatch_artifact() {
        with_host(|host| *host = super::super::HostState::default());
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
        with_host(|host| *host = super::super::HostState::default());
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
            fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));
        let mut bytes = [0u8; 0x100];
        Rdram::new(&mut bytes).store_h(0xffff_ffff_a000_0040, 0x1235);
        fn64_recomp_rs::set_write_observer(previous);

        assert_eq!(
            PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
            vec![(0x40, 2)]
        );
        assert_eq!(renderer_calls.get(), 1);
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    }

    fn unmapped(bank: BankId, pc: GuestPc, start: GuestPc, end: GuestPc) -> CpuFault {
        CpuFault {
            at: ExecutionKey::new(bank, pc),
            kind: CpuFaultKind::UnmappedPc {
                bank_start: start.get(),
                bank_end: end.get(),
            },
        }
    }

    fn rewrite_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        Err(unmapped(
            REWRITE_OLD_BANK,
            pc,
            REWRITE_ENTRY,
            GuestPc::new(REWRITE_ENTRY.get() + 0x34),
        ))
    }

    fn rewrite_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        rewrite_lookup(pc)
    }

    fn rewrite_interpreter_runner(
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let words = match entry.bank {
            REWRITE_OLD_BANK => REWRITE_A_WORDS,
            REWRITE_NEW_BANK => {
                REWRITE_B_ENTRIES.with(|entries| entries.borrow_mut().push(entry));
                REWRITE_B_WORDS
            }
            bank => {
                return BlockRun::new(
                    BlockExit::Fault(unmapped(
                        bank,
                        entry.pc,
                        REWRITE_ENTRY,
                        GuestPc::new(REWRITE_ENTRY.get() + 0x34),
                    )),
                    0,
                );
            }
        };
        let mut catalog = CodeCatalog::new();
        catalog
            .register(CodeBank::new(entry.bank, REWRITE_ENTRY, words.to_vec()).unwrap())
            .unwrap();
        let run =
            run_bank(&catalog, entry.bank, entry, budget, ctx, mem).unwrap_or_else(|unsupported| {
                panic!("rewrite interpreter hit unsupported op: {unsupported:?}")
            });
        match run.exit {
            BlockExit::ResolveTransfer { target_pc, .. }
                if ctx.is_thread_return(target_pc.get()) =>
            {
                BlockRun::new(BlockExit::ThreadReturn, run.instructions)
            }
            _ => run,
        }
    }

    fn rewrite_builder(
        bytes: &[u8],
        generation: u64,
    ) -> Result<(CodeBank, GeneratedBankRunner), String> {
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().push((generation, bytes.to_vec())));
        let expected = std::iter::once(0x1122_3344)
            .chain(REWRITE_A_WORDS.into_iter().skip(1))
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        if generation != 1 || bytes != expected {
            return Err(format!(
                "unexpected CPU rewrite generation/image: {generation} {bytes:02x?}"
            ));
        }
        Ok((
            CodeBank::new(REWRITE_NEW_BANK, REWRITE_ENTRY, REWRITE_B_WORDS.to_vec())
                .map_err(|error| error.to_string())?,
            GeneratedBankRunner::new(REWRITE_NEW_BANK, rewrite_interpreter_runner),
        ))
    }

    fn dma_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        Err(unmapped(
            DMA_OLD_BANK,
            pc,
            DMA_ENTRY,
            GuestPc::new(DMA_ENTRY.get() + 8),
        ))
    }

    fn dma_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        dma_lookup(pc)
    }

    fn dma_rewrite_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match (entry.bank, entry.pc) {
            (DMA_OLD_BANK, DMA_ENTRY) => {
                mem.store_w(0xFFFF_FFFF_A460_0000, DMA_PHYSICAL);
                mem.store_w(0xFFFF_FFFF_A460_0004, 0x1000_0020);
                mem.store_w(0xFFFF_FFFF_A460_000C, 7);
                BlockRun::new(BlockExit::Checkpoint(entry), 5)
            }
            (DMA_NEW_BANK, DMA_ENTRY) => {
                mem.store_w(0xFFFF_FFFF_8000_0014, 0xD00D_0001);
                BlockRun::new(
                    BlockExit::Transfer(ExecutionKey::new(
                        DMA_NEW_BANK,
                        GuestPc::new(DMA_ENTRY.get() + 4),
                    )),
                    1,
                )
            }
            (DMA_NEW_BANK, pc) if pc == GuestPc::new(DMA_ENTRY.get() + 4) => {
                mem.store_w(0xFFFF_FFFF_8000_0018, 0xD00D_0002);
                BlockRun::new(BlockExit::ThreadReturn, 1)
            }
            (bank, pc) => BlockRun::new(
                BlockExit::Fault(unmapped(
                    bank,
                    pc,
                    DMA_ENTRY,
                    GuestPc::new(DMA_ENTRY.get() + 8),
                )),
                0,
            ),
        }
    }

    fn dma_rewrite_builder(
        bytes: &[u8],
        generation: u64,
    ) -> Result<(CodeBank, GeneratedBankRunner), String> {
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().push((generation, bytes.to_vec())));
        if generation != 1 || bytes != [0x3c, 0x08, 0x12, 0x34, 0x35, 0x08, 0x56, 0x78] {
            return Err(format!(
                "unexpected DMA rewrite generation/image: {generation} {bytes:02x?}"
            ));
        }
        Ok((
            CodeBank::new(DMA_NEW_BANK, DMA_ENTRY, vec![1, 1])
                .map_err(|error| error.to_string())?,
            GeneratedBankRunner::new(DMA_NEW_BANK, dma_rewrite_runner),
        ))
    }

    fn live_host(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
        ctx.set_r32(2, 0x1234);
        mem.store_w(0xFFFF_FFFF_8000_0000, ctx.r_u32(2));
    }

    fn ordered_writer_host(_ctx: &mut RsContext, mem: &mut Rdram<'_>) {
        mem.as_mut_slice()[0x7000 ^ 3] = 1;
        super::super::suspend_active_coroutine(fn64_runtime::Yield::PauseSelf);
        mem.as_mut_slice()[0x7000 ^ 3] = 3;
    }

    fn ordered_writer_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            ORDERED_WRITER_ENTRY => BlockRun::new(
                BlockExit::HostCall {
                    vram: ORDERED_WRITER_HOST,
                    resume: ExecutionKey::new(ORDERED_WRITER_BANK, ORDERED_WRITER_RESUME),
                },
                1,
            ),
            ORDERED_WRITER_RESUME => BlockRun::new(BlockExit::ThreadReturn, 1),
            pc => panic!("unexpected ordered-writer test PC {pc}"),
        }
    }

    fn ordered_sync_host(_ctx: &mut RsContext, mem: &mut Rdram<'_>) {
        mem.as_mut_slice()[0x7200 ^ 3] = 1;
        track_rsp_execution_or_hle_mutation(mem.as_mut_slice(), |rdram| {
            rdram[0x7200 ^ 3] = 2;
        });
        track_rdp_renderer_mutation(mem.as_mut_slice(), |rdram| {
            rdram[0x7200 ^ 3] = 3;
        });
        mem.as_mut_slice()[0x7200 ^ 3] = 4;
    }

    fn ordered_sync_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            ORDERED_SYNC_ENTRY => BlockRun::new(
                BlockExit::HostCall {
                    vram: ORDERED_SYNC_HOST,
                    resume: ExecutionKey::new(ORDERED_SYNC_BANK, ORDERED_SYNC_RESUME),
                },
                1,
            ),
            ORDERED_SYNC_RESUME => BlockRun::new(BlockExit::ThreadReturn, 1),
            pc => panic!("unexpected ordered synchronous-writer test PC {pc}"),
        }
    }

    fn live_host_lookup(vram: u32) -> Option<RecompFunc> {
        (vram == LIVE_HOST.get()).then_some(live_host)
    }

    fn forbidden_catalog_legacy_lookup(_vram: u32) -> Option<RecompFunc> {
        panic!("canonical catalog consulted the legacy global host lookup")
    }

    fn live_test_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            LIVE_ENTRY => {
                assert_eq!(ctx.r_u64(20), 0xffff_ffff_cafe_babe);
                assert_eq!(ctx.hi, 0x1234);
                assert_eq!(ctx.lo, 0x5678);
                BlockRun::new(
                    BlockExit::ResolveCall {
                        source_bank: LIVE_BANK,
                        target_pc: LIVE_HOST,
                        resume: ExecutionKey::new(LIVE_BANK, LIVE_NEXT),
                    },
                    3,
                )
            }
            LIVE_NEXT => BlockRun::new(BlockExit::ThreadReturn, 2),
            pc => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(LIVE_BANK, pc),
                    kind: CpuFaultKind::UnmappedPc {
                        bank_start: LIVE_ENTRY.get(),
                        bank_end: LIVE_NEXT.get() + 4,
                    },
                }),
                0,
            ),
        }
    }

    fn catalog_rewrite_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.bank {
            CATALOG_REWRITE_A => {
                mem.store_w(0xffff_ffff_8000_0080, 0x2402_0002);
                BlockRun::new(
                    BlockExit::ExecutableWrite {
                        source_bank: CATALOG_REWRITE_A,
                        resume: ExecutionKey::new(CATALOG_REWRITE_A, CATALOG_REWRITE_ENTRY),
                    },
                    1,
                )
            }
            CATALOG_REWRITE_B => {
                mem.store_w(0xffff_ffff_8000_0010, 0x0000_beef);
                BlockRun::new(BlockExit::ThreadReturn, 1)
            }
            bank => unreachable!("unexpected catalog rewrite bank {bank}"),
        }
    }

    fn prepared_generation_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.bank {
            PREPARED_STATIC_BANK => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(PREPARED_GENERATION_BANK, PREPARED_GENERATION_ENTRY),
                    kind: CpuFaultKind::NoActiveGeneration,
                }),
                1,
            ),
            PREPARED_GENERATION_BANK => BlockRun::new(BlockExit::ThreadReturn, 1),
            bank => unreachable!("unexpected prepared-continuation bank {bank}"),
        }
    }

    fn live_entry_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        let key = ExecutionKey::new(LIVE_ACTIVE_BANK.with(std::cell::Cell::get), pc);
        if matches!(pc, LIVE_ENTRY | LIVE_NEXT) {
            Ok(key)
        } else {
            Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: LIVE_ENTRY.get(),
                    bank_end: LIVE_NEXT.get() + 4,
                },
            })
        }
    }

    fn live_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        live_entry_lookup(pc)
    }

    fn irq_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            IRQ_ENTRY => {
                ctx.set_r(4, 0x0010_0401); // OS_IM_PI
                os_set_int_mask(ctx, mem);
                mem.store_w(0xFFFF_FFFF_A460_0000, 0x400);
                mem.store_w(0xFFFF_FFFF_A460_0004, 0x1000_0020);
                mem.store_w(0xFFFF_FFFF_A460_000C, 3);
                BlockRun::new(
                    BlockExit::Checkpoint(ExecutionKey::new(IRQ_BANK, IRQ_RESUME)),
                    5,
                )
            }
            IRQ_VECTOR => {
                mem.store_w(0xFFFF_FFFF_8000_0000, ctx.cop0_epc);
                mem.store_w(0xFFFF_FFFF_8000_0004, ctx.cop0_cause);
                mem.store_w(0xFFFF_FFFF_8000_0008, ctx.cop0_status);
                mem.store_w(0xFFFF_FFFF_A460_0010, 1 << 1); // clear PI interrupt
                let resume = GuestPc::new(ctx.exception_return_pc());
                BlockRun::new(
                    BlockExit::Checkpoint(ExecutionKey::new(IRQ_BANK, resume)),
                    2,
                )
            }
            IRQ_RESUME => {
                mem.store_w(0xFFFF_FFFF_8000_000C, ctx.cop0_cause);
                mem.store_w(0xFFFF_FFFF_8000_0010, ctx.cop0_status);
                BlockRun::new(BlockExit::ThreadReturn, 2)
            }
            pc => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(IRQ_BANK, pc),
                    kind: CpuFaultKind::UnmappedPc {
                        bank_start: IRQ_ENTRY.get(),
                        bank_end: IRQ_VECTOR.get() + 4,
                    },
                }),
                0,
            ),
        }
    }

    fn irq_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        let key = ExecutionKey::new(IRQ_BANK, pc);
        if matches!(pc, IRQ_ENTRY | IRQ_RESUME | IRQ_VECTOR) {
            Ok(key)
        } else {
            Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: IRQ_ENTRY.get(),
                    bank_end: IRQ_VECTOR.get() + 4,
                },
            })
        }
    }

    fn irq_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        irq_lookup(pc)
    }

    fn timer_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            IRQ_ENTRY => {
                ctx.cop0_status = 1 | CpuInterruptLine::TIMER.cause_bit();
                ctx.write_cop0(9, 0);
                ctx.write_cop0(11, 2);
                BlockRun::new(
                    BlockExit::Checkpoint(ExecutionKey::new(TIMER_BANK, IRQ_RESUME)),
                    4,
                )
            }
            IRQ_VECTOR => {
                mem.store_w(0xFFFF_FFFF_8000_0020, ctx.cop0_epc);
                mem.store_w(0xFFFF_FFFF_8000_0024, ctx.cop0_cause);
                mem.store_w(0xFFFF_FFFF_8000_0028, ctx.cop0_count);
                ctx.write_cop0(11, ctx.cop0_compare);
                let resume = GuestPc::new(ctx.exception_return_pc());
                BlockRun::new(
                    BlockExit::Checkpoint(ExecutionKey::new(TIMER_BANK, resume)),
                    2,
                )
            }
            IRQ_RESUME => {
                mem.store_w(0xFFFF_FFFF_8000_002C, ctx.cop0_cause);
                mem.store_w(0xFFFF_FFFF_8000_0030, ctx.cop0_count);
                BlockRun::new(BlockExit::ThreadReturn, 2)
            }
            pc => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(TIMER_BANK, pc),
                    kind: CpuFaultKind::UnmappedPc {
                        bank_start: IRQ_ENTRY.get(),
                        bank_end: IRQ_VECTOR.get() + 4,
                    },
                }),
                0,
            ),
        }
    }

    fn timer_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        let key = ExecutionKey::new(TIMER_BANK, pc);
        if matches!(pc, IRQ_ENTRY | IRQ_RESUME | IRQ_VECTOR) {
            Ok(key)
        } else {
            Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: IRQ_ENTRY.get(),
                    bank_end: IRQ_VECTOR.get() + 4,
                },
            })
        }
    }

    fn timer_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        timer_lookup(pc)
    }

    #[test]
    fn c_adapter_round_trips_all_gprs_and_forces_zero() {
        let mut recompiled = RsContext::new();
        for i in 1..32 {
            recompiled.set_r(i, 0xA000_0000_0000_0000 | i as u64);
        }
        let mut c = c_from_recompiled(&recompiled);
        c.r0 = u64::MAX;
        c.r2 = 0x1234;
        copy_c_back(&c, &mut recompiled);
        assert_eq!(recompiled.r(0), 0);
        assert_eq!(recompiled.r(2), 0x1234);
        assert_eq!(recompiled.r(31), 0xA000_0000_0000_001F);
    }

    pub(super) unsafe extern "C" fn no_op_fpr_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` supplies its live stack-local C context.
        let ctx = unsafe { &mut *ctx };
        ctx.assert_float_mode_matches_status();
        let expected = if ctx.mips3_float_mode == 0 {
            // Safety: taking a union field address does not read that field.
            unsafe { &mut ctx.f0.u32_halves.1 as *mut u32 }
        } else {
            // Safety: taking a union field address does not read that field.
            unsafe { &mut ctx.f1.u32_halves.0 as *mut u32 }
        };
        assert_eq!(ctx.f_odd, expected);
    }

    pub(super) unsafe extern "C" fn write_f5_word_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` arms `f_odd` for this live context. N64Recomp's
        // generated odd-register expression for f5 is `(5 - 1) * 2`.
        unsafe { *(*ctx).f_odd.add(8) = 0xDEAD_BEEF };
    }

    pub(super) unsafe extern "C" fn change_fr_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` supplies its live stack-local C context.
        let ctx = unsafe { &mut *ctx };
        ctx.status_reg ^= STATUS_FR;
        ctx.mips3_float_mode ^= 1;
        ctx.arm_fpr_alias();
    }

    pub(super) unsafe extern "C" fn change_bev_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` supplies its live stack-local C context.
        unsafe { &mut *ctx }.status_reg ^= STATUS_BEV;
    }

    unsafe extern "C" fn transient_fr_write_shim(_rdram: *mut u8, ctx: *mut CContext) {
        TRANSIENT_FR_SHIM_ENTERED.store(true, Ordering::SeqCst);
        // Safety: the regression deliberately models a raw ABI shim which
        // changes to the other FPR view, accesses it, then restores the entry
        // mode before returning.
        let ctx = unsafe { &mut *ctx };
        let entry_status = ctx.status_reg;
        let entry_mode = ctx.mips3_float_mode;
        ctx.status_reg ^= STATUS_FR;
        ctx.mips3_float_mode ^= 1;
        ctx.arm_fpr_alias();
        // Safety: `arm_fpr_alias` made this pointer live for the transient
        // view. The generated odd-register expression for f5 is `(5-1)*2`.
        unsafe { *ctx.f_odd.add(8) = 0xA11C_E55E };
        ctx.status_reg = entry_status;
        ctx.mips3_float_mode = entry_mode;
        ctx.arm_fpr_alias();
    }

    fn patterned_fgr_state(tag: u64) -> PhysicalFgrState {
        PhysicalFgrState::from_words(std::array::from_fn(|idx| {
            let high = (tag >> 32) as u32 ^ (0x0101_0000 + idx as u32);
            let low = tag as u32 ^ (0x0000_0101 + idx as u32);
            (u64::from(high) << 32) | u64::from(low)
        }))
    }

    #[test]
    fn c_adapter_layout_is_reversible_and_mode_exact() {
        let physical = patterned_fgr_state(0xA5A5_5A5A_DEAD_BEEF);
        let words = physical.into_words();
        for fr in [false, true] {
            let mut source = RsContext::new();
            source.cop0_status = if fr { STATUS_FR } else { 0 };
            source.replace_physical_fgr_state(physical);
            let c = c_from_recompiled(&source);
            c.assert_float_mode_matches_status();
            let image = c.fpr_u64_bits();
            if fr {
                assert_eq!(image, words);
            } else {
                for pair in 0..16 {
                    let even = pair * 2;
                    let odd = even + 1;
                    assert_eq!(
                        image[even],
                        u64::from(words[even] as u32) | (u64::from(words[odd] as u32) << 32)
                    );
                    assert_eq!(
                        image[odd],
                        (words[even] >> 32) | (words[odd] & 0xFFFF_FFFF_0000_0000)
                    );
                }
            }

            let mut restored = RsContext::new();
            copy_c_back(&c, &mut restored);
            assert_eq!(restored.physical_fgr_state(), physical);
            assert_eq!(restored.cop0_status & STATUS_FR != 0, fr);
        }
    }

    #[test]
    fn c_adapter_noop_preserves_every_physical_fgr_in_both_fr_modes() {
        for (fr, bev) in [(false, false), (false, true), (true, false), (true, true)] {
            let expected = patterned_fgr_state(if fr {
                0xA5A5_5A5A_DEAD_BEEF
            } else {
                0x1122_3344_5566_7788
            });
            let mut ctx = RsContext::new();
            ctx.cop0_status = if fr { STATUS_FR } else { 0 } | if bev { STATUS_BEV } else { 0 };
            ctx.replace_physical_fgr_state(expected);
            let mut bytes = [];
            let mut mem = Rdram::new(&mut bytes);

            call_c(&mut ctx, &mut mem, "no_op_fpr_shim", no_op_fpr_shim);

            assert_eq!(ctx.physical_fgr_state(), expected, "FR={fr}");
            assert_eq!(ctx.cop0_status & STATUS_FR != 0, fr);
            assert_eq!(ctx.cop0_status & STATUS_BEV != 0, bev);
        }
    }

    #[test]
    fn c_adapter_rejects_bev_changes_before_status_copyback() {
        for entry_bev in [false, true] {
            let mut ctx = RsContext::new();
            ctx.cop0_status = if entry_bev { STATUS_BEV } else { 0 };
            let mut bytes = [];
            let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                call_c(
                    &mut ctx,
                    &mut Rdram::new(&mut bytes),
                    "change_bev_shim",
                    change_bev_shim,
                );
            }));
            assert!(rejected.is_err());
            assert_eq!(ctx.cop0_status & STATUS_BEV != 0, entry_bev);
        }
    }

    #[test]
    fn c_adapter_f_odd_write_targets_physical_fgr5_in_both_modes() {
        for fr in [false, true] {
            let initial = patterned_fgr_state(0x1234_5678_9ABC_DEF0).into_words();
            let mut ctx = RsContext::new();
            ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            ctx.replace_physical_fgr_state(PhysicalFgrState::from_words(initial));
            let mut bytes = [];
            call_c(
                &mut ctx,
                &mut Rdram::new(&mut bytes),
                "write_f5_word_shim",
                write_f5_word_shim,
            );
            let mut expected = initial;
            expected[5] = (expected[5] & 0xFFFF_FFFF_0000_0000) | 0xDEAD_BEEF;
            assert_eq!(ctx.physical_fgr_state().into_words(), expected, "FR={fr}");
        }
    }

    #[test]
    fn c_adapter_rejects_an_fr_transition_before_decoding_entry_view_bytes() {
        let expected = patterned_fgr_state(0x0BAD_F00D_CAFE_BABE);
        let mut ctx = RsContext::new();
        ctx.replace_physical_fgr_state(expected);
        let mut bytes = [];
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            call_c(
                &mut ctx,
                &mut Rdram::new(&mut bytes),
                "change_fr_shim",
                change_fr_shim,
            );
        }));
        assert!(rejected.is_err());
        assert_eq!(ctx.cop0_status & STATUS_FR, 0);
        assert_eq!(ctx.physical_fgr_state(), expected);
    }

    #[test]
    fn c_adapter_rejects_a_transient_fr_transition_before_the_shim_runs() {
        TRANSIENT_FR_SHIM_ENTERED.store(false, Ordering::SeqCst);
        let expected = patterned_fgr_state(0x1357_9BDF_2468_ACE0);
        let mut ctx = RsContext::new();
        ctx.replace_physical_fgr_state(expected);
        let mut bytes = [];
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            call_c(
                &mut ctx,
                &mut Rdram::new(&mut bytes),
                "transient_fr_write_shim",
                transient_fr_write_shim,
            );
        }));
        let panic = rejected.expect_err("unadmitted transient-FR shim must be rejected");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("registry rejection must use a string panic payload");
        assert!(
            message.contains("is not in the FR-stable adapter registry"),
            "unexpected rejection: {message}"
        );
        assert!(!TRANSIENT_FR_SHIM_ENTERED.load(Ordering::SeqCst));
        assert_eq!(ctx.cop0_status & STATUS_FR, 0);
        assert_eq!(ctx.physical_fgr_state(), expected);
    }

    #[test]
    fn c_adapter_float_helpers_return_through_f0_in_both_fr_modes() {
        let value = 0xFEDC_BA98_7654_3210u64;
        for fr in [false, true] {
            let initial = patterned_fgr_state(0xC001_D00D_A55A_5AA5).into_words();

            let mut float_ctx = RsContext::new();
            float_ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            float_ctx.replace_physical_fgr_state(PhysicalFgrState::from_words(initial));
            float_ctx.set_r(4, value >> 32);
            float_ctx.set_r(5, value as u32 as u64);
            let mut float_bytes = [];
            ull_to_f(&mut float_ctx, &mut Rdram::new(&mut float_bytes));
            assert_eq!(float_ctx.f_bits(0), (value as f32).to_bits(), "FR={fr}");
            let mut expected_float = initial;
            expected_float[0] =
                (expected_float[0] & 0xFFFF_FFFF_0000_0000) | u64::from((value as f32).to_bits());
            assert_eq!(
                float_ctx.physical_fgr_state().into_words(),
                expected_float,
                "FR={fr} float result changed non-result state"
            );

            let mut double_ctx = RsContext::new();
            double_ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            double_ctx.replace_physical_fgr_state(PhysicalFgrState::from_words(initial));
            double_ctx.set_r(4, value >> 32);
            double_ctx.set_r(5, value as u32 as u64);
            let mut double_bytes = [];
            ull_to_d(&mut double_ctx, &mut Rdram::new(&mut double_bytes));
            let result = (value as f64).to_bits();
            assert_eq!(double_ctx.d_bits(0), result, "FR={fr}");
            let mut expected_double = initial;
            if fr {
                expected_double[0] = result;
            } else {
                expected_double[0] =
                    (expected_double[0] & 0xFFFF_FFFF_0000_0000) | u64::from(result as u32);
                expected_double[1] = (expected_double[1] & 0xFFFF_FFFF_0000_0000) | (result >> 32);
            }
            assert_eq!(
                double_ctx.physical_fgr_state().into_words(),
                expected_double,
                "FR={fr} double result changed non-result state"
            );
        }
    }

    #[test]
    fn live_block_program_owns_thread_dispatch_and_charges_instruction_time() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
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
        let previous_host_lookup = fn64_recomp_rs::set_host_lookup(Some(live_host_lookup));

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
        fn64_recomp_rs::set_host_lookup(previous_host_lookup);
    }

    #[test]
    fn canonical_catalog_boot_owns_dispatch_host_lookup_and_evidence() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
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
            fn64_recomp_rs::set_host_lookup(Some(forbidden_catalog_legacy_lookup));
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
        assert!(fn64_recomp_rs::resolve_host_function(LIVE_HOST.get()).is_none());
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
        fn64_recomp_rs::set_host_lookup(previous_host_lookup);
    }

    #[test]
    fn canonical_catalog_scheduler_reaches_a_one_instruction_limit() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
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
        with_host(|host| *host = super::super::HostState::default());
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
        fn64_recomp_rs::notify_pi_dma_write(0x7100, 1);
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
        with_host(|host| *host = super::super::HostState::default());
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
        with_host(|host| *host = super::super::HostState::default());
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
            fn64_recomp_rs::set_host_lookup(Some(forbidden_catalog_legacy_lookup));
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
        fn64_recomp_rs::set_host_lookup(previous_host_lookup);
    }

    #[test]
    fn canonical_publication_binds_prepared_generation_continuation() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
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
    fn canonical_generation_cpu_write_retires_a_before_b_executes() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
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
        fn64_recomp_rs::notify_host_abi_write(0x80, 4);
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
        with_host(|host| *host = super::super::HostState::default());
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().clear());
        REWRITE_B_ENTRIES.with(|entries| entries.borrow_mut().clear());
        let mut bytes = vec![0u8; 0x200];
        fn64_recomp_rs::set_write_observer(None);
        fn64_recomp_rs::set_guest_write_boundary_observer(None);
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
        with_host(|host| *host = super::super::HostState::default());
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

    const BRK_BANK: BankId = BankId::new(0xB4EA);
    // Entry and the 0x8000_0180 general exception vector must sit in the same
    // registered code bank so the vectored handler PC is admitted; 33 words
    // from 0x8000_0100 spans [0x100, 0x184).
    const BRK_ENTRY: GuestPc = GuestPc::new(0x8000_0100);
    const BRK_VECTOR: GuestPc = GuestPc::new(0x8000_0180);

    // A block that hits a mid-function BREAK: the emitter renders this as
    // `BlockExit::Fault { kind: Exception { Breakpoint } }`. Before the driver
    // fix this reached `recompiled_gap_panic`; now it must vector to the
    // general exception handler like any architectural exception.
    fn brk_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            BRK_ENTRY => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(BRK_BANK, BRK_ENTRY),
                    kind: CpuFaultKind::Exception {
                        exception: fn64_recomp_rs::CpuException::Breakpoint,
                        epc: BRK_ENTRY,
                        branch_delay: false,
                        instruction_code: 0,
                        bad_vaddr: None,
                        coprocessor: None,
                    },
                }),
                1,
            ),
            BRK_VECTOR => {
                // Record the architectural state the vectoring produced so the
                // test can prove we reached the handler with a real BREAK frame.
                mem.store_w(0xFFFF_FFFF_8000_0000, ctx.cop0_epc);
                mem.store_w(0xFFFF_FFFF_8000_0004, ctx.cop0_cause);
                BlockRun::new(BlockExit::ThreadReturn, 1)
            }
            pc => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(BRK_BANK, pc),
                    kind: CpuFaultKind::UnmappedPc {
                        bank_start: BRK_ENTRY.get(),
                        bank_end: BRK_VECTOR.get() + 4,
                    },
                }),
                0,
            ),
        }
    }

    fn brk_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        let key = ExecutionKey::new(BRK_BANK, pc);
        if matches!(pc, BRK_ENTRY | BRK_VECTOR) {
            Ok(key)
        } else {
            Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: BRK_ENTRY.get(),
                    bank_end: BRK_ENTRY.get() + 4,
                },
            })
        }
    }

    fn brk_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        brk_lookup(pc)
    }

    fn canonical_brk_install() -> CatalogResolverInstallV1 {
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(BRK_BANK, BRK_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    BRK_BANK,
                    brk_runner,
                    ProgramArtifactIdentity::new([0xb4; 32]),
                ),
            )
            .unwrap();
        CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(BRK_BANK, BRK_ENTRY),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xea; 32]),
        )
    }

    fn assert_canonical_break_parks_with_post_exception_publication(thread_id: ThreadId) {
        assert!(crate::run_one_step());
        let checkpoint_publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(checkpoint)] = checkpoint_publications.as_slice()
        else {
            panic!("canonical BREAK did not first publish its exact charged checkpoint");
        };
        assert!(matches!(
            checkpoint.pending_exit,
            BlockExit::Fault(CpuFault {
                at: ExecutionKey { bank, pc },
                kind: CpuFaultKind::Exception {
                    exception: CpuException::Breakpoint,
                    ..
                },
            }) if bank == BRK_BANK && pc == BRK_ENTRY
        ));
        assert_eq!(checkpoint.prepared_continuation, None);

        assert!(crate::run_one_step());
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::ParkedFaultOpaque {
            thread,
            post_exception_cpu,
            fault,
            canonical_charged_instructions_at_publication,
        }] = publications.as_slice()
        else {
            panic!("canonical BREAK retained a stale exact publication: {publications:?}");
        };
        assert_eq!(*thread, thread_id);
        assert_eq!(*canonical_charged_instructions_at_publication, 1);
        assert_eq!(post_exception_cpu.cop0_epc, BRK_ENTRY.get());
        assert_eq!((post_exception_cpu.cop0_cause >> 2) & 0x1f, 9);
        assert!(matches!(
            fault,
            CpuFault {
                at: ExecutionKey { bank, pc },
                kind: CpuFaultKind::Exception {
                    exception: CpuException::Breakpoint,
                    ..
                },
            } if *bank == BRK_BANK && *pc == BRK_ENTRY
        ));
        assert!(!crate::is_thread_dead(thread_id));
        // The tested state is intentionally stopped forever; retire its
        // dormant coroutine while the caller's backing RDRAM is still live.
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
    }

    #[test]
    fn canonical_publication_static_break_replaces_exact_with_parked_fault() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x1000];
        let thread_id = 0xb4eb;

        // SAFETY: `bytes` remains live while the deliberately stopped thread
        // retains its dormant coroutine.
        unsafe {
            boot_thread0_catalog_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                canonical_brk_install(),
                test_boot_context(BRK_ENTRY),
                thread_id,
                10,
            );
        }
        with_host(|host| {
            host.thread_handle_vrams.insert(thread_id, 0x8000_0200);
        });

        assert_canonical_break_parks_with_post_exception_publication(thread_id);
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_publication_dynamic_break_replaces_exact_with_parked_fault() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x1000];
        let thread_id = 0xb4ec;

        // SAFETY: `bytes` remains live while the deliberately stopped thread
        // retains its dormant coroutine.
        unsafe {
            boot_thread0_catalog_program_with_dynamic_mapped_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                canonical_brk_install(),
                test_boot_context(BRK_ENTRY),
                thread_id,
                10,
            );
        }
        with_host(|host| {
            host.thread_handle_vrams.insert(thread_id, 0x8000_0200);
        });

        assert_canonical_break_parks_with_post_exception_publication(thread_id);
    }

    #[test]
    fn block_program_vectors_mid_function_break_instead_of_panicking() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x1000];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(BRK_BANK, BRK_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new(BRK_BANK, brk_runner),
            )
            .unwrap();
        let thread_id = 0xB4EA;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(BRK_BANK, BRK_ENTRY),
                test_boot_context(BRK_ENTRY),
                brk_lookup,
                brk_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        // Runs to completion — reaching the handler and returning — rather than
        // hitting recompiled_gap_panic on the BREAK fault. The entry block, the
        // vectored handler, and the thread-return retire across steps; drive the
        // executor until the thread is dead (bounded so a regression can't spin).
        let mut steps = 0;
        while !crate::is_thread_dead(thread_id) {
            assert!(
                crate::run_one_step(),
                "executor stalled before thread return"
            );
            steps += 1;
            assert!(
                steps < 8,
                "BREAK vectoring did not converge to thread return"
            );
        }

        let mem = Rdram::new(&mut bytes);
        // EPC captured the faulting PC, and Cause.ExcCode == 9 (Breakpoint).
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000) as u32, BRK_ENTRY.get());
        assert_eq!((mem.load_w(0xFFFF_FFFF_8000_0004) as u32 >> 2) & 0x1F, 9);
    }

    #[test]
    fn checkpoint_due_pi_enters_ip2_handler_before_the_next_guest_block() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        crate::load_rom_with_fixed_pi_latency(rom, 5);
        let mut bytes = vec![0u8; 0x1000];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(IRQ_BANK, IRQ_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new(IRQ_BANK, irq_runner),
            )
            .unwrap();
        let thread_id = 0x1A2;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(IRQ_BANK, IRQ_ENTRY),
                test_boot_context(IRQ_ENTRY),
                irq_lookup,
                irq_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 5);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400) as u32, 0x1234_5678);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000), 0);
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 7);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000) as u32, IRQ_RESUME.get());
            assert_eq!((mem.load_w(0xFFFF_FFFF_8000_0004) as u32 >> 2) & 0x1F, 0);
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_8000_0004) as u32 & CpuInterruptLine::RCP.cause_bit(),
                0
            );
            assert_ne!(mem.load_w(0xFFFF_FFFF_8000_0008) as u32 & (1 << 1), 0);
        }

        assert!(crate::run_one_step());
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(
                mem.load_w(0xFFFF_FFFF_8000_000C) as u32 & CpuInterruptLine::RCP.cause_bit(),
                0
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0010) as u32 & (1 << 1), 0);
        }
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[test]
    fn checkpoint_count_compare_match_enters_ip7_and_compare_write_acks_it() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x100];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(TIMER_BANK, IRQ_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new(TIMER_BANK, timer_runner),
            )
            .unwrap();
        let thread_id = 0x1A7;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(TIMER_BANK, IRQ_ENTRY),
                test_boot_context(IRQ_ENTRY),
                timer_lookup,
                timer_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 4);
        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 6);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0020) as u32, IRQ_RESUME.get());
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_8000_0024) as u32 & CpuInterruptLine::TIMER.cause_bit(),
                0
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0028) as u32, 2);
        }

        assert!(crate::run_one_step());
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(
                mem.load_w(0xFFFF_FFFF_8000_002C) as u32 & CpuInterruptLine::TIMER.cause_bit(),
                0
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0030) as u32, 3);
        }
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[test]
    fn status_adapters_are_per_context_state() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RsContext::new();
        ctx.set_r(4, 0x3400_0001);
        os_set_sr(&mut ctx, &mut mem);
        ctx.set_r(2, 0);
        os_get_sr(&mut ctx, &mut mem);
        assert_eq!(ctx.r_u32(2), 0x3400_0001);
    }

    #[test]
    fn typed_fpcsr_setter_and_new_thread_use_the_generated_cop1_authority() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut first = new_osthread_context(None);
        let mut second = new_osthread_context(None);

        assert_eq!(first.read_fcr(31), INITIAL_FPCSR);
        assert_eq!(second.read_fcr(31), INITIAL_FPCSR);

        first.set_r(4, 3);
        os_set_fpc_csr(&mut first, &mut mem);
        assert_eq!(first.r_u32(2), INITIAL_FPCSR);
        assert_eq!(first.read_fcr(31), 3);
        assert_eq!(second.read_fcr(31), INITIAL_FPCSR);

        second.set_r(4, 2);
        os_set_fpc_csr(&mut second, &mut mem);
        assert_eq!(second.r_u32(2), INITIAL_FPCSR);
        assert_eq!(second.read_fcr(31), 2);
        assert_eq!(first.read_fcr(31), 3);

        let pending: u32 = (1 << 16) | (1 << 11);
        first.set_r(4, u64::from(pending));
        let loud = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            os_set_fpc_csr(&mut first, &mut mem);
        }));
        assert!(
            loud.is_err(),
            "enabled Cause written by host call must stay loud"
        );
        assert_eq!(first.r_u32(2), 3);
        assert_eq!(first.read_fcr(31), pending);
        assert_eq!(second.read_fcr(31), 2);
    }

    /// Public osCreateThread gives each OSThread its own saved FPCSR. This
    /// drives real executor coroutine suspension and alternates A/B/A/B/A/B;
    /// the context-local values must survive switches through another thread.
    #[test]
    fn alternating_osthread_coroutines_preserve_independent_fpcsr() {
        const THREAD_A: ThreadId = 0xF5A0;
        const THREAD_B: ThreadId = 0xF5B0;

        let observed_a = Rc::new(RefCell::new(Vec::new()));
        let observed_b = Rc::new(RefCell::new(Vec::new()));
        let observed_a_body = Rc::clone(&observed_a);
        let observed_b_body = Rc::clone(&observed_b);

        with_executor(|exec| {
            exec.create_thread(THREAD_A, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = new_osthread_context(None);
                ctx.write_fcr(31, 3);
                observed_a_body.borrow_mut().push(ctx.read_fcr(31));
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_a_body.borrow_mut().push(ctx.read_fcr(31));
                ctx.write_fcr(31, 1);
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_a_body.borrow_mut().push(ctx.read_fcr(31));
            });
            exec.create_thread(THREAD_B, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = new_osthread_context(None);
                ctx.write_fcr(31, 2);
                observed_b_body.borrow_mut().push(ctx.read_fcr(31));
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_b_body.borrow_mut().push(ctx.read_fcr(31));
                ctx.write_fcr(31, 0);
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_b_body.borrow_mut().push(ctx.read_fcr(31));
            });
            exec.start_thread(THREAD_A);
            exec.start_thread(THREAD_B);
        });

        for _ in 0..6 {
            assert!(crate::run_one_step());
        }

        assert_eq!(&*observed_a.borrow(), &[3, 3, 1]);
        assert_eq!(&*observed_b.borrow(), &[2, 2, 0]);
        with_executor(|exec| {
            assert!(exec.is_thread_dead(THREAD_A));
            assert!(exec.is_thread_dead(THREAD_B));
        });
    }

    /// Thread 0 is the reset context, not an osCreateThread context. The
    /// public osInitialize contract performs the observable 0 -> FS|EV
    /// transition at the real typed boot entry.
    #[test]
    fn thread0_boot_path_transitions_fpcsr_only_at_os_initialize() {
        const THREAD0: ThreadId = 0xF500;
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom_with_fixed_pi_latency(Vec::new(), 1);
        BOOT_FPCSR_OBSERVATIONS.with(|observed| observed.borrow_mut().clear());
        let mut bytes = [0u8; 8];

        unsafe {
            boot_thread0(
                bytes.as_mut_ptr(),
                bytes.len(),
                evidence_lookup,
                observe_thread0_fpcsr_boot,
                THREAD0,
                10,
            );
        }
        crate::run_to_idle();

        BOOT_FPCSR_OBSERVATIONS.with(|observed| {
            assert_eq!(&*observed.borrow(), &[0, INITIAL_FPCSR]);
        });
        assert!(crate::is_thread_dead(THREAD0));
    }

    #[test]
    fn typed_os_initialize_replaces_the_current_context_fpcsr() {
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom_with_fixed_pi_latency(Vec::new(), 1);
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RsContext::new();
        ctx.write_fcr(31, 3);

        os_initialize(&mut ctx, &mut mem);

        assert_eq!(ctx.read_fcr(31), INITIAL_FPCSR);
    }

    #[test]
    fn created_osthread_enters_fr0_without_discarding_other_status_fields() {
        let inherited = 0xA5A5_5A5A | STATUS_FR;

        let ctx = new_osthread_context(Some(inherited));

        assert_eq!(ctx.cop0_status, inherited & !STATUS_FR);
        assert_eq!(ctx.read_fcr(31), INITIAL_FPCSR);
    }

    #[test]
    fn alternating_osthread_coroutines_preserve_all_physical_fgr_bits() {
        const THREAD_A: ThreadId = 0xF5C0;
        const THREAD_B: ThreadId = 0xF5D0;
        let state_a = patterned_fgr_state(0x1111_2222_3333_4444);
        let state_b = patterned_fgr_state(0xAAAA_BBBB_CCCC_DDDD);
        let observed_a = Rc::new(RefCell::new(Vec::new()));
        let observed_b = Rc::new(RefCell::new(Vec::new()));
        let observed_a_body = Rc::clone(&observed_a);
        let observed_b_body = Rc::clone(&observed_b);

        with_executor(|exec| {
            exec.create_thread(THREAD_A, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = RsContext::new();
                ctx.cop0_status &= !STATUS_FR;
                ctx.replace_physical_fgr_state(state_a);
                observed_a_body.borrow_mut().push(ctx.physical_fgr_state());
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_a_body.borrow_mut().push(ctx.physical_fgr_state());
            });
            exec.create_thread(THREAD_B, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = RsContext::new();
                ctx.cop0_status |= STATUS_FR;
                ctx.replace_physical_fgr_state(state_b);
                observed_b_body.borrow_mut().push(ctx.physical_fgr_state());
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_b_body.borrow_mut().push(ctx.physical_fgr_state());
            });
            exec.start_thread(THREAD_A);
            exec.start_thread(THREAD_B);
        });

        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert!(crate::run_one_step());

        assert_eq!(&*observed_a.borrow(), &[state_a, state_a]);
        assert_eq!(&*observed_b.borrow(), &[state_b, state_b]);
        with_executor(|exec| {
            assert!(exec.is_thread_dead(THREAD_A));
            assert!(exec.is_thread_dead(THREAD_B));
        });
    }

    #[test]
    fn typed_interrupt_masks_return_each_contexts_own_previous_value() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut first = RsContext::new();
        let mut second = RsContext::new();

        first.set_r(4, 0x0010_0401);
        os_set_int_mask(&mut first, &mut mem);
        assert_eq!(first.r_u32(2), 0);
        second.set_r(4, 0x0008_0401);
        os_set_int_mask(&mut second, &mut mem);
        assert_eq!(second.r_u32(2), 0);
        first.set_r(4, 0x0004_0401);
        os_set_int_mask(&mut first, &mut mem);
        assert_eq!(first.r_u32(2), 0x0010_0401);
    }

    #[test]
    fn typed_raw_word_accesses_and_sp_shims_share_one_device_fabric_state() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);

        mem.store_w(0xFFFF_FFFF_A408_0000, 0x0A8);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A408_0000) as u32, 0x0A8);

        let mut set = CContext::zeroed();
        set.r4 = 1 << 10;
        unsafe { crate::__osSpSetStatus_recomp(std::ptr::null_mut(), &mut set) };
        assert_eq!(mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & (1 << 7), 1 << 7);

        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_sp_dma_replaces_persistent_imem_on_guest_time() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let mut bytes = vec![0u8; 0x1000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut bytes);
            for (index, byte) in [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80]
                .into_iter()
                .enumerate()
            {
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(0x100 + index as u32),
                    byte,
                );
            }
        }
        with_host(|host| {
            host.runtime_rdram = bytes.as_mut_ptr();
            host.runtime_rdram_len = bytes.len();
        });
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        {
            let mut mem = Rdram::new(&mut bytes);
            mem.store_w(0xFFFF_FFFF_A404_0000, 0x1000);
            mem.store_w(0xFFFF_FFFF_A404_0004, 0x100);
            mem.store_w(0xFFFF_FFFF_A404_0008, 7);
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & fn64_runtime::SP_STATUS_DMA_BUSY,
                0
            );
        }

        crate::advance_virtual_time(8);
        {
            let mem = Rdram::new(&mut bytes);
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & fn64_runtime::SP_STATUS_DMA_BUSY,
                0
            );
        }
        crate::advance_virtual_time(9);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & 4, 0);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A400_1000) as u32, 0x1020_3040);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A400_1004) as u32, 0x5060_7080);
        }
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().sp_imem_generation),
            1
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_pi_registers_drive_the_live_timed_device_fabric() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        crate::load_rom_with_fixed_pi_latency(rom, 5);
        let mut bytes = vec![0u8; 0x1000];
        with_host(|host| {
            host.runtime_rdram = bytes.as_mut_ptr();
            host.runtime_rdram_len = bytes.len();
        });
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        {
            let mut mem = Rdram::new(&mut bytes);
            mem.store_w(0xFFFF_FFFF_A460_0000, 0x400);
            mem.store_w(0xFFFF_FFFF_A460_0004, 0x1000_0020);
            mem.store_w(0xFFFF_FFFF_A460_000C, 3);
            assert_eq!(
                mem.load_w(0xFFFF_FFFF_A460_0010) as u32,
                fn64_runtime::PI_STATUS_DMA_BUSY
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400), 0);
        }

        crate::advance_virtual_time(4);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400), 0);
        }
        crate::advance_virtual_time(5);

        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400) as u32, 0x1234_5678);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A460_0010), 0);
        assert_ne!(
            mem.load_w(0xFFFF_FFFF_A430_0008) as u32 & fn64_runtime::InterruptSource::Pi.bit(),
            0
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_rcp_acknowledgements_clear_the_shared_mi_sources() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let sources = [
            fn64_runtime::InterruptSource::Sp,
            fn64_runtime::InterruptSource::Si,
            fn64_runtime::InterruptSource::Ai,
            fn64_runtime::InterruptSource::Vi,
            fn64_runtime::InterruptSource::Dp,
        ];
        with_host(|host| {
            let fabric = &mut host.device_fabric;
            for source in sources {
                fabric.raise_interrupt(source);
            }
        });

        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        mem.store_w(0xFFFF_FFFF_A404_0010, 1 << 3);
        mem.store_w(0xFFFF_FFFF_A480_0018, 0);
        mem.store_w(0xFFFF_FFFF_A450_000C, 0);
        mem.store_w(0xFFFF_FFFF_A440_0010, 0);
        mem.store_w(0xFFFF_FFFF_A430_0000, 1 << 11);

        let pending = with_host(|host| host.device_fabric.snapshot().mi_pending);
        assert_eq!(pending & 0x3F, 0);
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_vi_registers_drive_half_line_timing_and_shared_mi() {
        crate::test_support::install_complete_render_backend(
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        );
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        mem.store_w(0xFFFF_FFFF_A440_0018, 525);
        mem.store_w(0xFFFF_FFFF_A440_000C, 100);
        crate::vi::arm_vi_retrace(1_000);

        crate::advance_virtual_time(190);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A440_0010), 98);
        crate::advance_virtual_time(191);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A440_0010), 100);
        assert_ne!(
            mem.load_w(0xFFFF_FFFF_A430_0008) as u32 & fn64_runtime::InterruptSource::Vi.bit(),
            0
        );

        mem.store_w(0xFFFF_FFFF_A440_0010, 0xFFFF_FFFF);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A440_0010), 100);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_A430_0008) as u32 & fn64_runtime::InterruptSource::Vi.bit(),
            0
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_ai_registers_schedule_the_live_guest_cycle_fifo() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        mem.store_w(0xFFFF_FFFF_A450_0008, 1);
        mem.store_w(0xFFFF_FFFF_A450_0010, 151);
        mem.store_w(0xFFFF_FFFF_A450_0000, 0x1000);
        mem.store_w(0xFFFF_FFFF_A450_0004, 0x80);
        assert_ne!(
            mem.load_w(0xFFFF_FFFF_A450_000C) as u32 & fn64_runtime::AI_STATUS_BUSY,
            0
        );
        let deadline = with_host(|host| host.device_fabric.next_deadline().unwrap().get());
        crate::advance_virtual_time(deadline);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_A450_000C) as u32,
            fn64_runtime::AI_STATUS_ENABLED
        );
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().mi_pending)
                & fn64_runtime::InterruptSource::Ai.bit(),
            0
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_si_registers_run_separate_timed_pif_write_and_read_dmas() {
        let mut bytes = vec![0u8; 0x200];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut bytes);
            for (offset, byte) in [(0, 1), (1, 3), (2, 0xFF), (3, 0), (6, 0xFE)] {
                view.write_u8(fn64_runtime::RdramAddr::from_offset(offset), byte);
            }
        }
        with_host(|host| {
            host.runtime_rdram = bytes.as_mut_ptr();
            host.runtime_rdram_len = bytes.len();
        });
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        {
            let mut mem = Rdram::new(&mut bytes);
            mem.store_w(0xFFFF_FFFF_A480_0000, 0);
            mem.store_w(0xFFFF_FFFF_A480_0010, 0);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A480_0018) & 1, 1);
        }
        crate::advance_virtual_time(1);
        {
            let mut mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A480_0018) as u32, 1 << 12);
            mem.store_w(0xFFFF_FFFF_A480_0018, 0);
            mem.store_w(0xFFFF_FFFF_A480_0000, 0);
            mem.store_w(0xFFFF_FFFF_A480_0004, 0);
        }
        crate::advance_virtual_time(2);
        let view = fn64_runtime::RdramView::from_storage(&bytes);
        assert_eq!(
            (3..6)
                .map(|offset| view.read_u8(fn64_runtime::RdramAddr::from_offset(offset)))
                .collect::<Vec<_>>(),
            vec![0x05, 0, 0]
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

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
        with_host(|host| *host = super::super::HostState::default());

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
        with_host(|host| *host = super::super::HostState::default());
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
        with_host(|host| *host = super::super::HostState::default());
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
