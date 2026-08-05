    use super::*;

mod programs;
use programs::mapped_observation_bank;

    #[test]
    fn generated_runner_runtime_receipts_preserve_v1_and_issue_source_complete_v2() {
        let v1 = generated_runner_runtime_source_receipt_v1();
        assert_eq!(v1.schema(), GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V1);
        assert_ne!(v1.source_sha256(), [0; 32]);
        assert_eq!(v1, generated_runner_runtime_source_receipt_v1());

        let v2 = generated_runner_runtime_source_receipt_v2();
        assert_eq!(v2.schema(), GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V2);
        assert_ne!(v2.source_sha256(), [0; 32]);
        assert_ne!(v2.source_sha256(), v1.source_sha256());
        assert_eq!(v2, generated_runner_runtime_source_receipt_v2());
        assert!(v2.typed_rdram());
        assert!(v2.typed_mmio());
        assert!(v2.typed_host_boundaries());
    }

    #[test]
    fn precompiled_image_admission_hashes_live_architectural_bytes_and_fails_closed() {
        let words = [0x3c1a_8003u32, 0x275a_6790, 0x0340_0008, 0];
        let expected: [u8; 32] = Sha256::digest(
            words
                .iter()
                .flat_map(|word| word.to_be_bytes())
                .collect::<Vec<_>>(),
        )
        .into();
        let mut storage = vec![0u8; 0x200];
        let mut mem = Rdram::new(&mut storage);
        for (index, word) in words.into_iter().enumerate() {
            mem.store_w(0xffff_ffff_8000_0180 + index as u64 * 4, word);
        }
        let bank = BankId::new(0x1234);
        assert_eq!(
            verify_precompiled_image(bank, GuestPc::new(0x8000_0180), 16, expected, &mem),
            Ok(())
        );

        mem.store_w(0xffff_ffff_8000_018c, 1);
        let miss = verify_precompiled_image(bank, GuestPc::new(0x8000_0180), 16, expected, &mem)
            .unwrap_err();
        assert_eq!(miss.expected_bank, bank);
        assert_ne!(miss.actual_sha256, miss.expected_sha256);
        assert!(miss
            .to_string()
            .starts_with("AotMiss for bank:0000000000001234"));
    }

    #[test]
    fn instruction_admission_ignores_neighbors_and_fails_before_a_changed_word() {
        let mut storage = vec![0u8; 0x200];
        let mut mem = Rdram::new(&mut storage);
        let pc = GuestPc::new(0x8000_0180);
        let bank = BankId::new(0x5678);
        mem.store_w(0xffff_ffff_8000_017c, 0xdead_beef);
        mem.store_w(0xffff_ffff_8000_0180, 0x2402_0001);
        mem.store_w(0xffff_ffff_8000_0184, 0xcafe_babe);
        assert_eq!(
            verify_precompiled_instruction_word(bank, pc, 0x2402_0001, &mem),
            Ok(())
        );

        mem.store_w(0xffff_ffff_8000_0180, 0x2402_0002);
        let miss = verify_precompiled_instruction_word(bank, pc, 0x2402_0001, &mem)
            .expect_err("changed fetched word must fail closed");
        assert_eq!(miss.expected_bank, bank);
        assert_eq!(miss.va_start, pc);
        assert_eq!(miss.byte_len, 4);
        assert_ne!(miss.actual_sha256, miss.expected_sha256);
    }

    #[test]
    fn synchronous_exception_entry_sets_epc_bd_exl_cause_and_vector() {
        let bank = BankId::new(7);
        let mut ctx = RecompContext::new();
        ctx.cop0_cause = 0x0000_0100; // preserve an unrelated pending bit
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_1004)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::Breakpoint,
                epc: GuestPc::new(0x8000_1000),
                branch_delay: true,
                instruction_code: 7,
                bad_vaddr: None,
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_epc, 0x8000_1000);
        assert_ne!(ctx.cop0_status & (1 << 1), 0);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 9);
        assert_ne!(ctx.cop0_cause & 0x100, 0);
    }

    #[test]
    fn floating_point_exception_enters_general_vector_with_exc_code_15() {
        let bank = BankId::new(0xF1);
        let mut ctx = RecompContext::new();
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_1804)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::FloatingPoint,
                epc: GuestPc::new(0x8000_1800),
                branch_delay: true,
                instruction_code: 0,
                bad_vaddr: None,
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_epc, 0x8000_1800);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 15);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
        assert_ne!(ctx.cop0_status & (1 << 1), 0);
    }

    #[test]
    fn nested_exception_preserves_first_epc_bd_and_bev_selects_boot_vector() {
        let bank = BankId::new(8);
        let mut ctx = RecompContext::new();
        ctx.cop0_status = (1 << 1) | (1 << 22); // EXL + BEV
        ctx.cop0_epc = 0x8000_2000;
        ctx.cop0_cause = 1 << 31;
        let nested = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_3000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::Syscall,
                epc: GuestPc::new(0x8000_3000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: None,
                coprocessor: None,
            },
        };

        assert_eq!(
            nested.enter_exception(&mut ctx),
            Some(GuestPc::new(0xBFC0_0380))
        );
        assert_eq!(ctx.cop0_epc, 0x8000_2000);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 8);
    }

    #[test]
    fn address_exception_commits_badvaddr_and_architectural_cause_code() {
        let bank = BankId::new(9);
        let mut ctx = RecompContext::new();
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_4000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorLoad,
                epc: GuestPc::new(0x8000_4000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(0x8000_0001),
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_badvaddr, 0x8000_0001);
        assert_eq!(ctx.cop0_epc, 0x8000_4000);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 4);
        assert_eq!(ctx.cop0_cause & (1 << 31), 0);
    }

    #[test]
    fn tlb_refill_commits_translation_registers_and_selects_refill_vector() {
        let bank = BankId::new(0x71);
        let mut ctx = RecompContext::new();
        ctx.cop0_context = 0xab80_0000;
        ctx.cop0_entry_hi = 0x0000_0042;
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_4000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::TlbRefillLoad,
                epc: GuestPc::new(0x8000_4000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(0x1234_5678),
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0000))
        );
        assert_eq!(ctx.cop0_badvaddr, 0x1234_5678);
        assert_eq!(ctx.cop0_context, 0xab89_1a20);
        assert_eq!(ctx.cop0_entry_hi, 0x1234_4042);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 2);

        let mut bev_ctx = RecompContext::new();
        bev_ctx.cop0_status = 1 << 22;
        assert_eq!(
            fault.enter_exception(&mut bev_ctx),
            Some(GuestPc::new(0xbfc0_0200))
        );
    }

    #[test]
    fn xtlb_refill_commits_full_translation_state_and_selects_extended_vector() {
        const BAD_VADDR: u64 = 0x4000_0088_7654_2040;
        let bank = BankId::new(0x73);
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_4000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::XTlbRefillLoad,
                epc: GuestPc::new(0x8000_4000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(BAD_VADDR),
                coprocessor: None,
            },
        };

        let mut ctx = RecompContext::new();
        ctx.cop0_context = 0xab80_0000;
        ctx.cop0_xcontext = 0x1234_5678_0000_0000;
        ctx.cop0_entry_hi = 0x51;
        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0080))
        );
        assert_eq!(ctx.cop0_badvaddr, BAD_VADDR);
        assert_eq!(ctx.cop0_context & 0xff80_0000, 0xab80_0000);
        assert_eq!(
            ctx.cop0_context & 0x007f_fff0,
            ((BAD_VADDR as u32) >> 9) & 0x007f_fff0
        );
        assert_eq!(
            ctx.cop0_xcontext & 0xffff_fffe_0000_0000,
            0x1234_5678_0000_0000 & 0xffff_fffe_0000_0000
        );
        assert_eq!((ctx.cop0_xcontext >> 31) & 0b11, BAD_VADDR >> 62);
        assert_eq!(
            (ctx.cop0_xcontext >> 4) & 0x07ff_ffff,
            (BAD_VADDR >> 13) & 0x07ff_ffff
        );
        assert_eq!(
            ctx.cop0_entry_hi,
            (BAD_VADDR & 0xc000_00ff_ffff_e000) | 0x51
        );
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 2);

        let mut bev_ctx = RecompContext::new();
        bev_ctx.cop0_status = 1 << 22;
        assert_eq!(
            fault.enter_exception(&mut bev_ctx),
            Some(GuestPc::new(0xbfc0_0280))
        );

        let mut nested = RecompContext::new();
        nested.cop0_status = 1 << 1;
        nested.cop0_epc = 0x8000_1234;
        assert_eq!(
            fault.enter_exception(&mut nested),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(nested.cop0_epc, 0x8000_1234);
        assert_eq!(nested.cop0_badvaddr, BAD_VADDR);
    }

    #[test]
    fn extended_address_error_retains_full_badvaddr_without_tlb_state_updates() {
        const BAD_VADDR: u64 = 0x9000_0001_0000_0040;
        let bank = BankId::new(0x74);
        let mut ctx = RecompContext::new();
        ctx.cop0_context = 0xabcd_1234;
        ctx.cop0_xcontext = 0x1234_5678_9abc_def0;
        ctx.cop0_entry_hi = 0x4000_0042;
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_4000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorStore,
                epc: GuestPc::new(0x8000_4000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(BAD_VADDR),
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_badvaddr, BAD_VADDR);
        assert_eq!(ctx.cop0_context, 0xabcd_1234);
        assert_eq!(ctx.cop0_xcontext, 0x1234_5678_9abc_def0);
        assert_eq!(ctx.cop0_entry_hi, 0x4000_0042);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 5);
    }

    #[test]
    fn invalid_modified_and_nested_refill_use_the_common_vector() {
        let bank = BankId::new(0x72);
        for (exception, expected_code) in [
            (CpuException::TlbInvalidStore, 3),
            (CpuException::TlbModified, 1),
        ] {
            let mut ctx = RecompContext::new();
            let fault = CpuFault {
                at: ExecutionKey::new(bank, GuestPc::new(0x8000_5000)),
                kind: CpuFaultKind::Exception {
                    exception,
                    epc: GuestPc::new(0x8000_5000),
                    branch_delay: false,
                    instruction_code: 0,
                    bad_vaddr: Some(0x0040_0000),
                    coprocessor: None,
                },
            };
            assert_eq!(
                fault.enter_exception(&mut ctx),
                Some(GuestPc::new(0x8000_0180))
            );
            assert_eq!((ctx.cop0_cause >> 2) & 0x1f, expected_code);
        }

        let mut nested = RecompContext::new();
        nested.cop0_status = 1 << 1;
        nested.cop0_epc = 0x8000_1234;
        let refill = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_6000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::TlbRefillStore,
                epc: GuestPc::new(0x8000_6000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(0xc001_2345),
                coprocessor: None,
            },
        };
        assert_eq!(
            refill.enter_exception(&mut nested),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(nested.cop0_epc, 0x8000_1234);
        assert_eq!(nested.cop0_badvaddr, 0xc001_2345);
    }

    #[test]
    fn nested_address_exception_updates_badvaddr_without_replacing_epc_or_bd() {
        let bank = BankId::new(10);
        let mut ctx = RecompContext::new();
        ctx.cop0_status = 1 << 1;
        ctx.cop0_epc = 0x8000_5000;
        ctx.cop0_cause = 1 << 31;
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_6004)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorStore,
                epc: GuestPc::new(0x8000_6000),
                branch_delay: true,
                instruction_code: 0,
                bad_vaddr: Some(0x8000_0002),
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_badvaddr, 0x8000_0002);
        assert_eq!(ctx.cop0_epc, 0x8000_5000);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 5);
    }

    #[test]
    fn coprocessor_unusable_exception_records_cause_ce() {
        let bank = BankId::new(11);
        for coprocessor in [0, 1] {
            let mut ctx = RecompContext::new();
            ctx.cop0_cause = 3 << 28;
            let fault = CpuFault {
                at: ExecutionKey::new(bank, GuestPc::new(0x8000_7000)),
                kind: CpuFaultKind::Exception {
                    exception: CpuException::CoprocessorUnusable,
                    epc: GuestPc::new(0x8000_7000),
                    branch_delay: false,
                    instruction_code: 0,
                    bad_vaddr: None,
                    coprocessor: Some(coprocessor),
                },
            };

            assert_eq!(
                fault.enter_exception(&mut ctx),
                Some(GuestPc::new(0x8000_0180))
            );
            assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 11);
            assert_eq!((ctx.cop0_cause >> 28) & 0b11, u32::from(coprocessor));
            assert_eq!(ctx.cop0_epc, 0x8000_7000);
        }
    }

    #[test]
    fn level_sensitive_interrupt_entry_obeys_ie_im_exl_and_erl() {
        let mut ctx = RecompContext::new();
        let interrupted = GuestPc::new(0x8000_1000);
        CpuInterruptLine::RCP.set_level(&mut ctx, true);
        assert_eq!(enter_pending_interrupt(&mut ctx, interrupted), None);
        assert_ne!(ctx.cop0_cause & CpuInterruptLine::RCP.cause_bit(), 0);

        ctx.cop0_status = 1 | CpuInterruptLine::RCP.cause_bit();
        ctx.cop0_cause |= (9 << 2) | (1 << 31);
        assert_eq!(
            enter_pending_interrupt(&mut ctx, interrupted),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_epc, interrupted.get());
        assert_ne!(ctx.cop0_status & (1 << 1), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 0);
        assert_eq!(ctx.cop0_cause & (1 << 31), 0);
        assert_ne!(ctx.cop0_cause & CpuInterruptLine::RCP.cause_bit(), 0);

        assert_eq!(enter_pending_interrupt(&mut ctx, interrupted), None);
        CpuInterruptLine::RCP.set_level(&mut ctx, false);
        assert_eq!(ctx.cop0_cause & CpuInterruptLine::RCP.cause_bit(), 0);
    }

    const VA: GuestPc = GuestPc::new(0x8000_1000);

    fn bank(id: u64, words: &[u32]) -> CodeBank {
        CodeBank::new(BankId::new(id), VA, words.to_vec()).unwrap()
    }

    fn instruction_entry_lo(physical_page: u32, valid: bool) -> u32 {
        ((physical_page >> 6) & 0x03ff_ffc0) | 1 | ((valid as u32) << 1) | (1 << 2)
    }

    fn map_instruction_pair(
        ctx: &mut RecompContext,
        virtual_pair: u32,
        even_physical: u32,
        odd_physical: u32,
        odd_valid: bool,
    ) {
        ctx.tlb_entries[0] = crate::runtime::TlbEntryRaw {
            page_mask: 0,
            entry_hi: u64::from(virtual_pair & 0xffff_e000),
            entry_lo0: instruction_entry_lo(even_physical, true),
            entry_lo1: instruction_entry_lo(odd_physical, odd_valid),
        };
    }

    fn first_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        ctx.set_r32(2, 1);
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    fn second_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        ctx.set_r32(2, 2);
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    fn catalog_dispatch_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        if entry.pc == VA {
            return BlockRun::new(
                BlockExit::ResolveCall {
                    source_bank: entry.bank,
                    target_pc: GuestPc::new(VA.get() + 4),
                    resume: ExecutionKey::new(entry.bank, GuestPc::new(VA.get() + 8)),
                },
                1,
            );
        }
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    fn catalog_dispatch_host(_ctx: &mut RecompContext, _mem: &mut Rdram<'_>) {}

    fn catalog_budget_runner(
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        ctx.set_r32(2, budget.get().try_into().unwrap());
        BlockRun::new(BlockExit::Yield(entry), budget.get())
    }

    fn catalog_test_program(
        id: BankId,
        runner: GeneratedBankFn,
        artifact_byte: u8,
    ) -> BlockProgram {
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(id, VA, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    id,
                    runner,
                    ProgramArtifactIdentity::new([artifact_byte; 32]),
                ),
            )
            .unwrap();
        program
    }

    #[test]
    fn catalog_block_program_captures_canonical_evidence_and_fixed_execution() {
        let id = BankId::new(0xc001);
        let entry = ExecutionKey::new(id, VA);
        let budget = InstructionBudget::new(2).unwrap();
        let program = catalog_test_program(id, first_runner, 0x11);
        let expected_evidence = program.evidence_snapshot();
        let catalog = CatalogBlockProgramV1::new(program, entry, budget).unwrap();

        assert_eq!(catalog.entry(), entry);
        assert_eq!(catalog.budget(), budget);
        assert_eq!(catalog.evidence(), &expected_evidence);
        assert_eq!(catalog.identity(), expected_evidence.identity);
        assert_eq!(catalog.build_receipt(), static_execution_build_receipt());
        assert!(catalog.reserves_bank(id));
        assert!(!catalog.reserves_bank(BankId::new(0xc0ff)));
        assert_eq!(catalog.resolve_entry(VA).unwrap(), entry);
        assert_eq!(catalog.resolve_transfer(id, VA).unwrap(), entry);

        let mut storage = [];
        let mut memory = Rdram::new(&mut storage);
        let mut context = RecompContext::new();
        assert_eq!(catalog.run(&mut context, &mut memory).instructions, 1);
        assert_eq!(context.r_u32(2), 1);
        assert_eq!(catalog.copy_execution_destinations()[0].destination, entry);
    }

    #[test]
    fn catalog_block_program_rejects_unadmitted_entry_and_unidentified_runner() {
        let id = BankId::new(0xc002);
        let hole = ExecutionKey::new(id, GuestPc::new(VA.get() + 8));
        assert!(matches!(
            CatalogBlockProgramV1::new(
                catalog_test_program(id, first_runner, 0x22),
                hole,
                InstructionBudget::new(2).unwrap(),
            ),
            Err(CatalogBlockProgramErrorV1::EntryNotAdmitted(CpuFault {
                kind: CpuFaultKind::UnmappedPc { .. },
                ..
            }))
        ));

        let mut unidentified = BlockProgram::new();
        unidentified
            .register(
                CodeBank::new(id, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new(id, first_runner),
            )
            .unwrap();
        assert!(matches!(
            CatalogBlockProgramV1::new(
                unidentified,
                ExecutionKey::new(id, VA),
                InstructionBudget::new(2).unwrap(),
            ),
            Err(CatalogBlockProgramErrorV1::MissingRunnerArtifactIdentity { bank })
                if bank == id
        ));
    }

    #[test]
    fn catalog_block_program_replacement_is_validated_before_installation() {
        let first = BankId::new(0xc003);
        let second = BankId::new(0xc004);
        let budget = InstructionBudget::new(2).unwrap();
        let mut catalog = CatalogBlockProgramV1::new(
            catalog_test_program(first, first_runner, 0x33),
            ExecutionKey::new(first, VA),
            budget,
        )
        .unwrap();
        let first_identity = catalog.identity();

        assert!(catalog
            .replace_program(
                catalog_test_program(second, second_runner, 0x44),
                ExecutionKey::new(second, GuestPc::new(VA.get() + 8)),
            )
            .is_err());
        assert_eq!(catalog.identity(), first_identity);
        assert_eq!(catalog.entry(), ExecutionKey::new(first, VA));

        catalog
            .replace_program(
                catalog_test_program(second, second_runner, 0x44),
                ExecutionKey::new(second, VA),
            )
            .unwrap();
        assert_ne!(catalog.identity(), first_identity);
        let mut storage = [];
        let mut memory = Rdram::new(&mut storage);
        let mut context = RecompContext::new();
        catalog.run(&mut context, &mut memory);
        assert_eq!(context.r_u32(2), 2);
    }

    #[test]
    fn catalog_block_dispatch_prefers_host_call_over_overlapping_guest_code() {
        let bank = BankId::new(0xc005);
        let entry = ExecutionKey::new(bank, VA);
        let mut block_program = BlockProgram::new();
        block_program
            .register(
                CodeBank::new(bank, VA, vec![0, 0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    catalog_dispatch_runner,
                    ProgramArtifactIdentity::new([0x55; 32]),
                ),
            )
            .unwrap();
        let program =
            CatalogBlockProgramV1::new(block_program, entry, InstructionBudget::new(4).unwrap())
                .unwrap();
        let target = GuestPc::new(VA.get() + 4);
        let resume = ExecutionKey::new(bank, GuestPc::new(VA.get() + 8));
        let hosts =
            HostFunctionCatalogV1::new(vec![(target.get(), catalog_dispatch_host)]).unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();

        assert_eq!(
            program.resolve_transfer(bank, target),
            Ok(ExecutionKey::new(bank, target)),
            "the host target must also be admitted guest code for this precedence regression"
        );

        assert_eq!(
            program
                .dispatch_exposing_exceptions_at(entry, &hosts, &mut ctx, &mut mem)
                .unwrap()
                .exit,
            BlockExit::HostCall {
                vram: target,
                resume,
            }
        );

        let no_hosts = HostFunctionCatalogV1::new(Vec::new()).unwrap();
        assert_eq!(
            program
                .dispatch_exposing_exceptions_at(entry, &no_hosts, &mut ctx, &mut mem)
                .unwrap()
                .exit,
            BlockExit::Yield(ExecutionKey::new(bank, target))
        );
    }

    #[test]
    fn catalog_block_dispatch_accepts_an_explicit_slice_budget() {
        let bank = BankId::new(0xc006);
        let entry = ExecutionKey::new(bank, VA);
        let program = CatalogBlockProgramV1::new(
            catalog_test_program(bank, catalog_budget_runner, 0x56),
            entry,
            InstructionBudget::new(4).unwrap(),
        )
        .unwrap();
        let hosts = HostFunctionCatalogV1::new(Vec::new()).unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();

        let one = program
            .dispatch_exposing_exceptions_at_budget(
                entry,
                &hosts,
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
            )
            .unwrap();
        assert_eq!(one.instructions, 2);
        assert_eq!(ctx.r_u32(2), 2);
        assert_eq!(program.budget().get(), 4);

        let installed = program
            .dispatch_exposing_exceptions_at(entry, &hosts, &mut ctx, &mut mem)
            .unwrap();
        assert_eq!(installed.instructions, 4);
        assert_eq!(ctx.r_u32(2), 4);
    }

    #[test]
    fn catalog_reservation_includes_physical_code_banks() {
        let static_bank = BankId::new(0xc007);
        let physical_bank = BankId::new(0xc008);
        let mut block_program = catalog_test_program(static_bank, first_runner, 0x57);
        block_program
            .register_physical_code(mapped_observation_bank(physical_bank))
            .unwrap();
        let program = CatalogBlockProgramV1::new(
            block_program,
            ExecutionKey::new(static_bank, VA),
            InstructionBudget::new(2).unwrap(),
        )
        .unwrap();

        assert!(program.reserves_bank(static_bank));
        assert!(program.reserves_bank(physical_bank));
        assert!(!program.reserves_bank(BankId::new(0xc009)));
    }

    #[test]
    fn catalog_resolution_reserves_inactive_generation_banks_until_digest_activation() {
        let first = BankId::new(0xc101);
        let second = BankId::new(0xc102);
        let static_bank = BankId::new(0xc103);
        let static_pc = GuestPc::new(VA.get() + 0x100);
        let image_a = 0x2402_0001u32.to_be_bytes();
        let image_b = 0x2402_0002u32.to_be_bytes();
        let mut block_program = BlockProgram::new();
        for (bank, pc, word, identity) in [
            (first, VA, 0x2402_0001, 0x81),
            (second, VA, 0x2402_0002, 0x82),
            (static_bank, static_pc, 0, 0x83),
        ] {
            block_program
                .register(
                    CodeBank::new(bank, pc, vec![word]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        first_runner,
                        ProgramArtifactIdentity::new([identity; 32]),
                    ),
                )
                .unwrap();
        }
        let program = CatalogBlockProgramV1::new(
            block_program,
            ExecutionKey::new(static_bank, static_pc),
            InstructionBudget::new(4).unwrap(),
        )
        .unwrap();
        let mut catalog = crate::generation::PrecompiledGenerationCatalog::new();
        for (id, bank, bytes) in [(1, first, image_a), (2, second, image_b)] {
            catalog
                .register(
                    crate::generation::PrecompiledGeneration::new(
                        crate::generation::GenerationId::new(id),
                        VA,
                        GuestPc::new(VA.get() + 4),
                        VA,
                        GuestPc::new(VA.get() + 4),
                        Sha256::digest(bytes).into(),
                        vec![crate::generation::PrecompiledShard::new(
                            bank,
                            VA,
                            GuestPc::new(VA.get() + 4),
                        )
                        .unwrap()],
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let backing = |id| {
            crate::generation::PrecompiledGenerationBackingV1::new(
                crate::generation::GenerationId::new(id),
                vec![crate::generation::BackedExecutableSpanV1::new(VA, 0x100, 4).unwrap()],
            )
            .unwrap()
        };
        let mut generations = crate::generation::BackedPrecompiledGenerationCatalogV1::new(
            catalog,
            vec![backing(2), backing(1)],
        )
        .unwrap();
        program
            .validate_precompiled_generations(&generations)
            .unwrap();
        assert!(program.reserves_bank_with_generations(first, &generations));
        assert!(program.reserves_bank_with_generations(second, &generations));
        assert!(program.reserves_bank_with_generations(static_bank, &generations));
        assert!(!program.reserves_bank_with_generations(BankId::new(0xc104), &generations));

        assert!(matches!(
            program.resolve_entry_with_generations(VA, &generations),
            Err(CpuFault {
                kind: CpuFaultKind::NoActiveGeneration,
                ..
            })
        ));
        generations
            .activate_for_fetch_with_physical(VA, |physical| {
                image_a[usize::try_from(physical - 0x100).unwrap()]
            })
            .unwrap();
        assert_eq!(
            program
                .resolve_entry_with_generations(VA, &generations)
                .unwrap(),
            ExecutionKey::new(first, VA)
        );
        assert_eq!(
            program
                .resolve_transfer_with_generations(second, static_pc, &generations)
                .unwrap(),
            ExecutionKey::new(static_bank, static_pc)
        );
    }

    fn zero_progress_executable_write_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let resume = ExecutionKey::new(entry.bank, GuestPc::new(VA.get() + 12));
        let exit = match entry.pc.get() - VA.get() {
            0 => BlockExit::ExecutableWrite {
                source_bank: entry.bank,
                resume,
            },
            4 => BlockExit::ExecutableWriteResolveCall {
                source_bank: entry.bank,
                target_pc: GuestPc::new(0x8000_2000),
                resume,
            },
            8 => BlockExit::ExecutableWriteFault(CpuFault::instruction_address_error(entry)),
            _ => unreachable!("test runner received an unexpected entry"),
        };
        BlockRun::new(exit, 0)
    }

    fn observation_transfer_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let first_bank = BankId::new(0x501);
        let second_bank = BankId::new(0x502);
        match entry {
            key if key == ExecutionKey::new(first_bank, VA) => BlockRun::new(
                BlockExit::Transfer(ExecutionKey::new(first_bank, GuestPc::new(VA.get() + 4))),
                1,
            ),
            key if key == ExecutionKey::new(first_bank, GuestPc::new(VA.get() + 4)) => {
                BlockRun::new(
                    BlockExit::ResolveTransfer {
                        source_bank: first_bank,
                        target_pc: VA,
                    },
                    1,
                )
            }

            key if key == ExecutionKey::new(second_bank, VA) => {
                BlockRun::new(BlockExit::Yield(key), 1)
            }
            _ => unreachable!("observation runner received unexpected destination {entry}"),
        }
    }

    fn observation_host_call_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let bank = BankId::new(0x503);
        match entry.pc {
            pc if pc == VA => BlockRun::new(
                BlockExit::HostCall {
                    vram: GuestPc::new(0x8000_4000),
                    resume: ExecutionKey::new(bank, GuestPc::new(VA.get() + 4)),
                },
                1,
            ),
            pc if pc == GuestPc::new(VA.get() + 4) => BlockRun::new(BlockExit::Yield(entry), 1),
            _ => unreachable!("host-call runner received unexpected destination {entry}"),
        }
    }

    fn observation_image_changed_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        if entry.pc == VA {
            return BlockRun::new(
                BlockExit::Transfer(ExecutionKey::new(entry.bank, GuestPc::new(VA.get() + 4))),
                3,
            );
        }
        let miss = AotMiss {
            expected_bank: entry.bank,
            va_start: VA,
            byte_len: 8,
            expected_sha256: [0x11; 32],
            actual_sha256: [0x22; 32],
            first_diff_offset: None,
        };
        BlockRun::new(BlockExit::ImageChanged { at: entry, miss }, 0)
    }

    #[test]
    fn resolves_an_interior_instruction_without_a_function_entry() {
        let mut catalog = CodeCatalog::new();
        catalog
            .register(bank(1, &[0x1111, 0x2222, 0x3333]))
            .unwrap();

        let key = ExecutionKey::new(BankId::new(1), GuestPc::new(VA.get() + 4));
        assert_eq!(catalog.resolve(key).unwrap().word, 0x2222);
    }

    #[test]
    fn static_transfer_resolution_prefers_an_admitting_source_bank() {
        let first = BankId::new(0xd001);
        let second = BankId::new(0xd002);
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(first.get(), &[1, 2])).unwrap();
        catalog.register(bank(second.get(), &[3, 4])).unwrap();
        let target = GuestPc::new(VA.get() + 4);

        assert_eq!(
            catalog.resolve_transfer(second, target).unwrap(),
            ExecutionKey::new(second, target)
        );
        assert!(matches!(
            catalog.resolve_entry(first, target),
            Err(CpuFault {
                at,
                kind: CpuFaultKind::AmbiguousPc {
                    first_candidate,
                    second_candidate,
                    candidate_count: 2,
                },
            }) if at == ExecutionKey::new(first, target)
                && first_candidate == first
                && second_candidate == second
        ));
    }

    #[test]
    fn catalog_resolver_policy_evidence_is_implementation_issued_and_build_bound() {
        let evidence = catalog_resolver_policy_evidence_v1();
        assert_eq!(evidence.policy(), CATALOG_RESOLVER_POLICY_NAME_V1);
        assert_eq!(
            evidence.exception_vectors(),
            &CATALOG_RESOLVER_EXCEPTION_VECTORS_V1
        );
        assert!(evidence.aligned_pc_admission());
        assert!(evidence.exact_active_owner_resolution());
        assert!(evidence.explicit_thread_return_boundary());
        assert!(evidence.misaligned_target_fault());
        assert!(evidence.unmapped_or_ambiguous_target_fault());
        assert!(evidence.traps_enter_shared_resolver());
        assert_eq!(evidence.build_receipt(), static_execution_build_receipt());
    }

    #[test]
    fn static_transfer_resolution_admits_one_cross_bank_target() {
        let source = BankId::new(0xd010);
        let destination = BankId::new(0xd011);
        let target = GuestPc::new(0x8000_2000);
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(source.get(), &[1])).unwrap();
        catalog
            .register(CodeBank::new(destination, target, vec![2]).unwrap())
            .unwrap();

        assert_eq!(
            catalog.resolve_transfer(source, target).unwrap(),
            ExecutionKey::new(destination, target)
        );
        assert_eq!(
            catalog.resolve_entry(source, target).unwrap(),
            ExecutionKey::new(destination, target)
        );
    }

    #[test]
    fn static_resolution_reports_ordered_complete_ambiguity() {
        let first = BankId::new(0xd020);
        let second = BankId::new(0xd021);
        let third = BankId::new(0xd022);
        let fault_bank = BankId::new(0xd0ff);
        let mut catalog = CodeCatalog::new();
        for id in [third, first, second] {
            catalog.register(bank(id.get(), &[1])).unwrap();
        }

        assert!(matches!(
            catalog.resolve_entry(fault_bank, VA),
            Err(CpuFault {
                at,
                kind: CpuFaultKind::AmbiguousPc {
                    first_candidate,
                    second_candidate,
                    candidate_count: 3,
                },
            }) if at == ExecutionKey::new(fault_bank, VA)
                && first_candidate == first
                && second_candidate == second
        ));
    }

    #[test]
    fn static_resolution_fails_typed_for_unmapped_unknown_and_misaligned_targets() {
        let known = BankId::new(0xd030);
        let unknown = BankId::new(0xd031);
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(known.get(), &[1])).unwrap();
        let unmapped = GuestPc::new(0x8000_3000);

        assert!(matches!(
            catalog.resolve_transfer(known, unmapped),
            Err(CpuFault {
                at,
                kind: CpuFaultKind::UnmappedPc { bank_start, bank_end },
            }) if at == ExecutionKey::new(known, unmapped)
                && bank_start == VA.get()
                && bank_end == VA.get() + 4
        ));
        assert!(matches!(
            catalog.resolve_entry(unknown, unmapped),
            Err(CpuFault {
                at,
                kind: CpuFaultKind::UnknownBank,
            }) if at == ExecutionKey::new(unknown, unmapped)
        ));

        let misaligned = GuestPc::new(VA.get() + 2);
        assert_eq!(
            catalog.resolve_transfer(known, misaligned),
            Err(CpuFault::instruction_address_error(ExecutionKey::new(
                known, misaligned,
            )))
        );
        assert_eq!(
            catalog.resolve_entry(unknown, misaligned),
            Err(CpuFault::instruction_address_error(ExecutionKey::new(
                unknown, misaligned,
            )))
        );
    }

    #[test]
    fn executable_region_rewrite_retires_stale_bank_and_runner_atomically() {
        let first = BankId::new(0x101);
        let second = BankId::new(0x102);
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(VA, GuestPc::new(VA.get() + 4));
        let mut storage = [0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();

        assert_eq!(
            region
                .install(
                    &mut program,
                    CodeBank::new(first, VA, vec![0x2402_0001]).unwrap(),
                    GeneratedBankRunner::new(first, first_runner),
                )
                .unwrap(),
            None
        );
        let first_key = region.resolve(VA).unwrap();
        assert_eq!(
            program
                .run(
                    first_key,
                    InstructionBudget::new(2).unwrap(),
                    &mut ctx,
                    &mut mem,
                )
                .instructions,
            1
        );
        assert_eq!(ctx.r_u32(2), 1);

        assert_eq!(
            region
                .install(
                    &mut program,
                    CodeBank::new(second, VA, vec![0x2402_0002]).unwrap(),
                    GeneratedBankRunner::new(second, second_runner),
                )
                .unwrap(),
            Some(first)
        );
        assert_eq!(region.active_bank(), Some(second));
        assert!(matches!(
            program
                .run(
                    first_key,
                    InstructionBudget::new(2).unwrap(),
                    &mut ctx,
                    &mut mem,
                )
                .exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnknownBank,
                ..
            })
        ));
        let second_key = region.resolve(VA).unwrap();
        assert_eq!(second_key.bank, second);
        program.run(
            second_key,
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert_eq!(ctx.r_u32(2), 2);
    }

    #[test]
    fn same_virtual_address_resolves_by_bank_identity() {
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(1, &[0x1111])).unwrap();
        catalog.register(bank(2, &[0x2222])).unwrap();

        let first = ExecutionKey::new(BankId::new(1), VA);
        let second = ExecutionKey::new(BankId::new(2), VA);
        assert_eq!(catalog.resolve(first).unwrap().word, 0x1111);
        assert_eq!(catalog.resolve(second).unwrap().word, 0x2222);
    }

    #[test]
    fn sparse_bank_sorts_spans_and_never_resolves_a_bounding_hole() {
        let id = BankId::new(3);
        let bank = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, GuestPc::new(VA.get() + 0x20), vec![0x3333]).unwrap(),
                CodeSpan::new(id, VA, vec![0x1111, 0x2222]).unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(bank.vram_start(), VA);
        assert_eq!(bank.vram_end(), GuestPc::new(VA.get() + 0x24));
        assert_eq!(bank.instruction_count(), 3);

        let mut catalog = CodeCatalog::new();
        catalog.register(bank).unwrap();
        assert_eq!(
            catalog
                .resolve(ExecutionKey::new(id, GuestPc::new(VA.get() + 0x20)))
                .unwrap()
                .word,
            0x3333
        );
        assert!(matches!(
            catalog
                .resolve(ExecutionKey::new(id, GuestPc::new(VA.get() + 0x10)))
                .unwrap_err()
                .kind,
            CpuFaultKind::UnmappedPc { .. }
        ));
    }

    #[test]
    fn sparse_bank_rejects_overlap_and_cross_bank_spans() {
        let id = BankId::new(4);
        let overlap = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, VA, vec![1, 2]).unwrap(),
                CodeSpan::new(id, GuestPc::new(VA.get() + 4), vec![3]).unwrap(),
            ],
        );
        assert_eq!(
            overlap,
            Err(BankError::OverlappingSpans {
                bank: id,
                left_end: GuestPc::new(VA.get() + 8),
                right_start: GuestPc::new(VA.get() + 4),
            })
        );

        let other = BankId::new(5);
        assert_eq!(
            CodeBank::from_spans(id, vec![CodeSpan::new(other, VA, vec![1]).unwrap()]),
            Err(BankError::SpanBankMismatch {
                bank: id,
                span_bank: other,
                start: VA,
            })
        );
    }

    #[test]
    fn classify_uses_sparse_admission_and_rejects_holes() {
        let id = BankId::new(6);
        let bank = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, VA, vec![0x2402_0001]).unwrap(),
                CodeSpan::new(id, GuestPc::new(VA.get() + 0x20), vec![0x0100_0008]).unwrap(),
            ],
        )
        .unwrap();
        let mut catalog = CodeCatalog::new();
        catalog.register(bank).unwrap();
        assert_eq!(
            catalog.classify(ExecutionKey::new(id, VA)).unwrap(),
            BankWordKind::Straight
        );
        assert_eq!(
            catalog
                .classify(ExecutionKey::new(id, GuestPc::new(VA.get() + 0x20)))
                .unwrap(),
            BankWordKind::ControlTransfer
        );
        assert!(matches!(
            catalog.classify(ExecutionKey::new(id, GuestPc::new(VA.get() + 0x10))),
            Err(CpuFault {
                kind: CpuFaultKind::UnmappedPc { .. },
                ..
            })
        ));
    }

#[cfg(test)]
mod aot_miss_offset_tests {
    use super::super::{AotMiss, BankId, GuestPc};

    #[test]
    fn a_digest_only_miss_reports_no_offset_and_says_nothing_extra() {
        // The seams that hold only a digest must not invent a location.
        let miss = AotMiss {
            expected_bank: BankId::new(7),
            va_start: GuestPc::new(0x8000_1000),
            byte_len: 0x40,
            expected_sha256: [0x11; 32],
            actual_sha256: [0x22; 32],
            first_diff_offset: None,
        };
        let rendered = miss.to_string();
        assert!(!rendered.contains("first differing byte"), "{rendered}");
    }

    #[test]
    fn a_located_miss_names_the_offset_and_its_address() {
        // This is the whole point: "which byte" distinguishes a game writing
        // to a data field inside the image from a different overlay entirely.
        let miss = AotMiss {
            expected_bank: BankId::new(7),
            va_start: GuestPc::new(0x8000_1000),
            byte_len: 0x40,
            expected_sha256: [0x11; 32],
            actual_sha256: [0x22; 32],
            first_diff_offset: Some(0x24),
        };
        let rendered = miss.to_string();
        assert!(rendered.contains("first differing byte at +0x24"), "{rendered}");
        assert!(rendered.contains("0x80001024"), "{rendered}");
    }
}
