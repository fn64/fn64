use super::*;

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
