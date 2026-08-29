    use super::*;
    use fn64_cpu_runtime::{
        run_bank, BackedExecutableSpanV1, BlockRun, BootCicIdentity, BootCop0Context, BootRegion,
        CodeBank, CodeCatalog, CpuFaultKind, GeneratedBankRunner, GenerationId,
        PhysicalCodeBank, PrecompiledGeneration, PrecompiledGenerationBackingV1, PrecompiledShard,
        Sha256Digest, BOOT_CONTEXT_SCHEMA_V1,
    };
    use sha2::Digest;
    use std::sync::atomic::{AtomicBool, Ordering};
    #[cfg(feature = "dynamic-mapped-runtime")]
    use std::sync::atomic::AtomicUsize;

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
        fn64_cpu_runtime::RDRAM_LEN
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
            at: fn64_runtime::EmulatedInstant::new(100 + sequence as u64),
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
            at: fn64_runtime::EmulatedInstant::new(100 + sequence as u64),
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
            at: fn64_runtime::EmulatedInstant::new(100 + sequence as u64),
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


    struct PublicSiRuntimeStateTestReset;


    impl Drop for PublicSiRuntimeStateTestReset {
        fn drop(&mut self) {
            with_executor(|executor| *executor = fn64_runtime::Executor::new());
            with_host(|host| *host = crate::HostState::default());
            PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
            PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
            EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow_mut().clear());
            CPU_INSTRUCTION_STORE_TRACE.with(|trace| *trace.borrow_mut() = None);
            RDP_RENDERER_WRITER_TRACE.with(|trace| *trace.borrow_mut() = None);
            fn64_cpu_runtime::set_write_observer(None);
            fn64_cpu_runtime::set_guest_write_boundary_observer(None);
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
            *host = crate::HostState::default();
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
        fn64_cpu_runtime::set_write_observer(Some(record_executable_and_renderer_write));
        fn64_cpu_runtime::set_guest_write_boundary_observer(Some(classify_live_executable_write));
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        crate::load_rom(rom);
        fn64_runtime::SiDmaRequest {
            kind: fn64_runtime::SiDmaKind::PifToDram,
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6000),
        }
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


    fn patterned_fgr_state(tag: u64) -> PhysicalFgrState {
        PhysicalFgrState::from_words(std::array::from_fn(|idx| {
            let high = (tag >> 32) as u32 ^ (0x0101_0000 + idx as u32);
            let low = tag as u32 ^ (0x0000_0101 + idx as u32);
            (u64::from(high) << 32) | u64::from(low)
        }))
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
                        exception: fn64_cpu_runtime::CpuException::Breakpoint,
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
        with_host(|host| *host = crate::HostState::default());
    }

mod bootstrap_writers;
mod canonical_install;
mod evidence_live_a;
mod live_program_b;
mod c_adapter;
mod exceptions_fabric;
mod mutation_state;
mod resident_boundary;

// `is_test_c_shim` in `runners.rs` names these as `tests::<shim>`, which it
// could do directly while every test lived in one inline `mod tests`. They
// now live in the `c_adapter` submodule, so re-export them under the same
// path the caller already uses.
pub(in crate::recompiled) use c_adapter::{change_bev_shim, change_fr_shim, no_op_fpr_shim, write_f5_word_shim};
