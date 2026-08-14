use super::*;
#[allow(unused_imports)]
use super::super::*;
    
    #[cfg(feature = "dev-interpreter")]
    use crate::semantic::{
        AlignedDirectWordAddress, CartridgeReadOutcome, CartridgeStoreOutcome, CartridgeWordPort,
    };

    #[test]
    fn block_program_registration_is_atomic_and_bank_qualified() {
        let first = BankId::new(10);
        let second = BankId::new(11);
        let mut program = BlockProgram::new();
        assert_eq!(
            program.register(
                bank(10, &[0x1111]),
                GeneratedBankRunner::new(second, first_runner),
            ),
            Err(ProgramError::RunnerBankMismatch {
                code_bank: first,
                runner_bank: second,
            })
        );
        assert!(program.code().bank(first).is_none());

        program
            .register(
                bank(10, &[0x1111]),
                GeneratedBankRunner::new(first, first_runner),
            )
            .unwrap();
        program
            .register(
                bank(11, &[0x2222]),
                GeneratedBankRunner::new(second, second_runner),
            )
            .unwrap();
        assert_eq!(
            program.register(
                bank(10, &[0x3333]),
                GeneratedBankRunner::new(first, first_runner),
            ),
            Err(ProgramError::DuplicateBank { bank: first })
        );

        let mut bytes = [];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        let budget = InstructionBudget::new(2).unwrap();
        let first_key = ExecutionKey::new(first, VA);
        let second_key = ExecutionKey::new(second, VA);
        assert_eq!(
            program
                .run(first_key, budget, &mut ctx, &mut mem)
                .instructions,
            1
        );
        assert_eq!(ctx.r_u32(2), 1);
        assert_eq!(
            program
                .run(second_key, budget, &mut ctx, &mut mem)
                .instructions,
            1
        );
        assert_eq!(ctx.r_u32(2), 2);
    }

    #[test]
    fn block_program_observes_direct_transferred_and_resolved_entries_in_order() {
        let first_bank = BankId::new(0x501);
        let second_bank = BankId::new(0x502);
        let first_artifact = ProgramArtifactIdentity::new([0x51; 32]);
        let second_artifact = ProgramArtifactIdentity::new([0x52; 32]);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(first_bank, VA, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    first_bank,
                    observation_transfer_runner,
                    first_artifact,
                ),
            )
            .unwrap();
        program
            .register(
                CodeBank::new(second_bank, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    second_bank,
                    observation_transfer_runner,
                    second_artifact,
                ),
            )
            .unwrap();

        let immutable_before = program.evidence_snapshot();
        assert!(program.copy_execution_destinations().is_empty());
        assert!(program
            .code()
            .resolve(ExecutionKey::new(first_bank, VA))
            .is_ok());
        assert!(program.copy_execution_destinations().is_empty());

        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let mut resolver = |source_bank: BankId, target_pc: GuestPc| {
            assert_eq!(source_bank, first_bank);
            assert_eq!(target_pc, VA);
            Ok(ExecutionKey::new(second_bank, target_pc))
        };
        let run = program
            .dispatch(
                ExecutionKey::new(first_bank, VA),
                InstructionBudget::new(6).unwrap(),
                &mut ctx,
                &mut mem,
                &mut resolver,
            )
            .unwrap();
        assert_eq!(
            run.exit,
            BlockExit::Yield(ExecutionKey::new(second_bank, VA))
        );
        assert_eq!(
            program.copy_execution_destinations(),
            vec![
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(first_bank, VA),
                    runner_artifact_identity: Some(first_artifact),
                    instructions: 1,
                },
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(first_bank, GuestPc::new(VA.get() + 4),),
                    runner_artifact_identity: Some(first_artifact),
                    instructions: 1,
                },
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(second_bank, VA),
                    runner_artifact_identity: Some(second_artifact),
                    instructions: 1,
                },
            ]
        );
        assert_eq!(
            immutable_before,
            program.evidence_snapshot(),
            "historical execution must not enter future-affecting program evidence"
        );
    }

    #[test]
    fn block_program_records_host_resume_only_when_guest_execution_reenters() {
        let bank = BankId::new(0x503);
        let artifact = ProgramArtifactIdentity::new([0x53; 32]);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, VA, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    observation_host_call_runner,
                    artifact,
                ),
            )
            .unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let mut resolver = |_source_bank: BankId, _target_pc: GuestPc| {
            unreachable!("host-call fixture must not resolve a guest transfer")
        };

        let first = program
            .dispatch(
                ExecutionKey::new(bank, VA),
                InstructionBudget::new(4).unwrap(),
                &mut ctx,
                &mut mem,
                &mut resolver,
            )
            .unwrap();
        let resume = match first.exit {
            BlockExit::HostCall { resume, .. } => resume,
            exit => panic!("expected host call, got {exit:?}"),
        };
        assert_eq!(program.copy_execution_destinations().len(), 1);

        let second = program
            .dispatch(
                resume,
                InstructionBudget::new(4).unwrap(),
                &mut ctx,
                &mut mem,
                &mut resolver,
            )
            .unwrap();
        assert_eq!(second.exit, BlockExit::Yield(resume));
        assert_eq!(
            program.copy_execution_destinations(),
            vec![
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(bank, VA),
                    runner_artifact_identity: Some(artifact),
                    instructions: 1,
                },
                ExecutionDestinationObservation {
                    destination: resume,
                    runner_artifact_identity: Some(artifact),
                    instructions: 1,
                },
            ]
        );
    }

    #[test]
    fn image_change_preserves_prior_progress_without_recording_stale_entry() {
        let bank = BankId::new(0x504);
        let artifact = ProgramArtifactIdentity::new([0x54; 32]);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, VA, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    observation_image_changed_runner,
                    artifact,
                ),
            )
            .unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let mut resolver = |_source_bank: BankId, _target_pc: GuestPc| {
            unreachable!("the image-change fixture uses only direct transfers")
        };
        let run = program
            .dispatch(
                ExecutionKey::new(bank, VA),
                InstructionBudget::new(6).unwrap(),
                &mut ctx,
                &mut mem,
                &mut resolver,
            )
            .unwrap();
        assert_eq!(run.instructions, 3);
        assert!(matches!(
            run.exit,
            BlockExit::ImageChanged {
                at: ExecutionKey { pc, .. },
                ..
            } if pc == GuestPc::new(VA.get() + 4)
        ));
        assert_eq!(
            program.copy_execution_destinations(),
            vec![ExecutionDestinationObservation {
                destination: ExecutionKey::new(bank, VA),
                runner_artifact_identity: Some(artifact),
                instructions: 3,
            }]
        );
    }

    #[test]
    fn block_program_observation_lifetime_is_explicit_and_program_local() {
        let bank = BankId::new(0x504);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new(bank, first_runner),
            )
            .unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        program.run(
            ExecutionKey::new(bank, VA),
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert_eq!(
            program.copy_execution_destinations(),
            vec![ExecutionDestinationObservation {
                destination: ExecutionKey::new(bank, VA),
                runner_artifact_identity: None,
                instructions: 1,
            }]
        );
        assert!(BlockProgram::new().copy_execution_destinations().is_empty());
        program.clear_execution_destinations();
        assert!(program.copy_execution_destinations().is_empty());
        assert!(program.code().bank(bank).is_some());

        program.set_execution_destination_history_limit(NonZeroUsize::new(2));
        for _ in 0..3 {
            program.run(
                ExecutionKey::new(bank, VA),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
            );
        }
        assert_eq!(program.copy_execution_destinations().len(), 2);
        program.set_execution_destination_history_enabled(false);
        program.run(
            ExecutionKey::new(bank, VA),
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert!(program.copy_execution_destinations().is_empty());
        program.set_execution_destination_history_enabled(true);
        program.run(
            ExecutionKey::new(bank, VA),
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert_eq!(program.copy_execution_destinations().len(), 1);
    }

    pub(super) fn mapped_observation_bank(bank: BankId) -> PhysicalCodeBank {
        PhysicalCodeBank::from_spans(
            bank,
            vec![
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0000_0040, vec![0x4022_4800]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0010_0000, vec![0x2402_0001]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0010_0ffc, vec![0x1000_0001]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0020_0000, vec![0x2402_0002]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0030_0000, vec![0x2403_0003]).unwrap(),
            ],
        )
        .unwrap()
    }

    #[cfg(feature = "dev-interpreter")]
    struct MappedProgramCartridge {
        reads: u32,
    }

    #[cfg(feature = "dev-interpreter")]
    impl CartridgeWordPort for MappedProgramCartridge {
        fn read_w(&mut self, address: AlignedDirectWordAddress) -> CartridgeReadOutcome {
            if address.get() != 0xffff_ffff_b000_0000 {
                return CartridgeReadOutcome::NotCartridge;
            }
            self.reads += 1;
            CartridgeReadOutcome::Handled(0x1357_9bdf)
        }

        fn classify_store_w(
            &mut self,
            _address: AlignedDirectWordAddress,
        ) -> CartridgeStoreOutcome {
            CartridgeStoreOutcome::NotCartridge
        }
    }

    #[cfg(feature = "dev-interpreter")]
    #[test]
    fn block_program_mapped_fallback_uses_injected_cartridge_memory_port() {
        let bank = BankId::new(0x50a);
        let mut program = BlockProgram::new();
        program
            .register_physical_code(PhysicalCodeBank::new(bank, 0x40, vec![0x8c82_0000]).unwrap())
            .unwrap();
        let entry = ExecutionKey::new(bank, GuestPc::new(0x8000_0040));
        let mut storage = [0u8; 0x100];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        ctx.set_r32(4, 0xb000_0000u32 as i32);
        let mut cartridge = MappedProgramCartridge { reads: 0 };
        let mut no_mmio = NoMmio;
        let run = program.run_with_memory_port(
            entry,
            InstructionBudget::new(1).unwrap(),
            &mut ctx,
            &mut mem,
            &mut MemoryPort::new(&mut no_mmio, &mut cartridge),
        );
        assert_eq!(run.instructions, 1);
        assert_eq!(ctx.r(2), 0x1357_9bdf);
        assert_eq!(cartridge.reads, 1);
        assert_eq!(
            program.copy_execution_destinations(),
            vec![ExecutionDestinationObservation {
                destination: entry,
                runner_artifact_identity: None,
                instructions: 1,
            }]
        );
    }

    #[cfg(feature = "dev-interpreter")]
    struct UnusedResolver;

    #[cfg(feature = "dev-interpreter")]
    impl TransferResolver for UnusedResolver {
        fn resolve(
            &mut self,
            _source_bank: BankId,
            _target_pc: GuestPc,
        ) -> Result<ExecutionKey, CpuFault> {
            panic!("thread-return test must not resolve a guest transfer")
        }

        fn resolve_call(
            &mut self,
            _source_bank: BankId,
            _target_pc: GuestPc,
            _resume: ExecutionKey,
        ) -> Result<CallResolution, CpuFault> {
            panic!("thread-return test must not resolve a guest call")
        }
    }

    #[cfg(feature = "dev-interpreter")]
    #[test]
    fn block_program_dispatch_threads_cartridge_port_through_mapped_delay_slot() {
        let bank = BankId::new(0x50b);
        let mut program = BlockProgram::new();
        program
            .register_physical_code(
                PhysicalCodeBank::new(bank, 0x40, vec![0x03e0_0008, 0x8c82_0000]).unwrap(),
            )
            .unwrap();
        let mut storage = [0u8; 0x100];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        ctx.set_r32(4, 0xb000_0000u32 as i32);
        ctx.set_r32(31, 0xffff_fffcu32 as i32);
        ctx.set_thread_return_pc(Some(0xffff_fffc));
        let mut cartridge = MappedProgramCartridge { reads: 0 };
        let mut no_mmio = NoMmio;
        let dispatched = program
            .dispatch_with_memory_port(
                ExecutionKey::new(bank, GuestPc::new(0x8000_0040)),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                &mut UnusedResolver,
                &mut MemoryPort::new(&mut no_mmio, &mut cartridge),
            )
            .unwrap();
        assert_eq!(dispatched.exit, BlockExit::ThreadReturn);
        assert_eq!(dispatched.instructions, 2);
        assert_eq!(ctx.r(2), 0x1357_9bdf);
        assert_eq!(cartridge.reads, 1);
    }

    #[test]
    fn mapped_fetch_failures_do_not_record_an_entered_destination() {
        let bank = BankId::new(0x505);
        let mut program = BlockProgram::new();
        program
            .register_physical_code(mapped_observation_bank(bank))
            .unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let budget = InstructionBudget::new(2).unwrap();

        let mut misaligned_ctx = RecompContext::new();
        let misaligned = program.run(
            ExecutionKey::new(bank, GuestPc::new(0x8000_0042)),
            budget,
            &mut misaligned_ctx,
            &mut mem,
        );
        assert!(matches!(
            misaligned.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::Exception {
                    exception: CpuException::AddressErrorLoad,
                    ..
                },
                ..
            })
        ));

        let mut refill_ctx = RecompContext::new();
        let refill = program.run(
            ExecutionKey::new(bank, GuestPc::new(0x0060_0000)),
            budget,
            &mut refill_ctx,
            &mut mem,
        );
        assert!(matches!(
            refill.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::Exception {
                    exception: CpuException::TlbRefillLoad,
                    ..
                },
                ..
            })
        ));

        let mut unmapped_ctx = RecompContext::new();
        map_instruction_pair(
            &mut unmapped_ctx,
            0x0080_0000,
            0x0040_0000,
            0x0040_1000,
            true,
        );
        let unmapped = program.run(
            ExecutionKey::new(bank, GuestPc::new(0x0080_0000)),
            budget,
            &mut unmapped_ctx,
            &mut mem,
        );
        assert!(matches!(
            unmapped.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnmappedPhysicalInstruction { .. },
                ..
            })
        ));

        let mut delay_ctx = RecompContext::new();
        map_instruction_pair(&mut delay_ctx, 0x0040_0000, 0x0010_0000, 0x0030_0000, false);
        let delay = program.run(
            ExecutionKey::new(bank, GuestPc::new(0x0040_0ffc)),
            budget,
            &mut delay_ctx,
            &mut mem,
        );
        assert!(matches!(
            delay.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::Exception {
                    exception: CpuException::TlbInvalidLoad,
                    branch_delay: true,
                    ..
                },
                ..
            })
        ));
        assert!(program.copy_execution_destinations().is_empty());
    }

    #[test]
    fn mapped_history_records_only_admitted_units_with_honest_lane_identity() {
        let bank = BankId::new(0x506);
        let mut program = BlockProgram::new();
        program
            .register_physical_code(mapped_observation_bank(bank))
            .unwrap();
        let aot_artifact = ProgramArtifactIdentity::new([0x56; 32]);
        let direct_aot_entry = GuestPc::new(0x8010_0000);
        let aot = MappedAotBlock::new(
            program.physical_code(),
            &RecompContext::new(),
            bank,
            direct_aot_entry,
            &[0x2402_0001],
            GeneratedBankRunner::new_with_artifact_identity(bank, first_runner, aot_artifact),
        )
        .unwrap();
        program.register_mapped_aot(aot).unwrap();

        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let budget = InstructionBudget::new(2).unwrap();
        let mut ctx = RecompContext::new();
        let interpreted_entry = GuestPc::new(0x8000_0040);
        let interpreted = program.run(
            ExecutionKey::new(bank, interpreted_entry),
            budget,
            &mut ctx,
            &mut mem,
        );
        assert!(matches!(interpreted.exit, BlockExit::Fault(_)));
        program.run(
            ExecutionKey::new(bank, direct_aot_entry),
            budget,
            &mut ctx,
            &mut mem,
        );
        assert_eq!(
            program.copy_execution_destinations(),
            vec![
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(bank, interpreted_entry),
                    runner_artifact_identity: None,
                    instructions: interpreted.instructions,
                },
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(bank, direct_aot_entry),
                    runner_artifact_identity: Some(aot_artifact),
                    instructions: 1,
                },
            ]
        );

        let mut stale_program = BlockProgram::new();
        stale_program
            .register_physical_code(mapped_observation_bank(bank))
            .unwrap();
        let stale_entry = GuestPc::new(0x0080_0000);
        let mut original_ctx = RecompContext::new();
        map_instruction_pair(
            &mut original_ctx,
            stale_entry.get(),
            0x0010_0000,
            0x0030_0000,
            true,
        );
        let stale = MappedAotBlock::new(
            stale_program.physical_code(),
            &original_ctx,
            bank,
            stale_entry,
            &[0x2402_0001],
            GeneratedBankRunner::new_with_artifact_identity(bank, first_runner, aot_artifact),
        )
        .unwrap();
        stale_program.register_mapped_aot(stale).unwrap();
        let mut remapped_ctx = RecompContext::new();
        map_instruction_pair(
            &mut remapped_ctx,
            stale_entry.get(),
            0x0020_0000,
            0x0030_0000,
            true,
        );
        let stale_run = stale_program.run(
            ExecutionKey::new(bank, stale_entry),
            budget,
            &mut remapped_ctx,
            &mut mem,
        );
        assert!(matches!(
            stale_run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::StaleInstructionIdentity { .. },
                ..
            })
        ));
        assert!(stale_program.copy_execution_destinations().is_empty());
    }

    #[test]
    fn mapped_wraparound_delay_fetch_is_precise_and_records_only_after_admission() {
        let bank = BankId::new(0x507);
        let branch_word = 0x1000_0001;
        let delay_word = 0x2442_0005;
        let physical = PhysicalCodeBank::from_spans(
            bank,
            vec![
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0010_0ffc, vec![branch_word]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0020_0000, vec![delay_word]).unwrap(),
            ],
        )
        .unwrap();
        let mut program = BlockProgram::new();
        program.register_physical_code(physical).unwrap();
        let entry = GuestPc::new(0xffff_fffc);
        let budget = InstructionBudget::new(2).unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);

        let mut invalid_ctx = RecompContext::new();
        for tlb in &mut invalid_ctx.tlb_entries {
            tlb.entry_hi = 0x0040_0000;
        }
        invalid_ctx.tlb_entries[0] = crate::runtime::TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0xffff_e000,
            entry_lo0: instruction_entry_lo(0x0010_0000, true),
            entry_lo1: instruction_entry_lo(0x0010_0000, true),
        };
        invalid_ctx.tlb_entries[1] = crate::runtime::TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0,
            entry_lo0: instruction_entry_lo(0x0020_0000, false),
            entry_lo1: instruction_entry_lo(0x0020_1000, false),
        };
        let invalid = program.run(
            ExecutionKey::new(bank, entry),
            budget,
            &mut invalid_ctx,
            &mut mem,
        );
        assert!(matches!(
            invalid.exit,
            BlockExit::Fault(CpuFault {
                at: ExecutionKey {
                    pc: GuestPc(0),
                    ..
                },
                kind: CpuFaultKind::Exception {
                    exception: CpuException::TlbInvalidLoad,
                    epc,
                    branch_delay: true,
                    bad_vaddr: Some(0),
                    ..
                },
            }) if epc == entry
        ));
        assert!(program.copy_execution_destinations().is_empty());

        let mut valid_ctx = invalid_ctx;
        valid_ctx.tlb_entries[1].entry_lo0 = instruction_entry_lo(0x0020_0000, true);
        let valid = program.run(
            ExecutionKey::new(bank, entry),
            budget,
            &mut valid_ctx,
            &mut mem,
        );
        assert_eq!(valid.instructions, 2);
        assert_eq!(
            valid.exit,
            BlockExit::ResolveTransfer {
                source_bank: bank,
                target_pc: GuestPc::new(4),
            }
        );
        assert_eq!(valid_ctx.r_u32(2), 5);
        assert_eq!(
            program.copy_execution_destinations(),
            vec![ExecutionDestinationObservation {
                destination: ExecutionKey::new(bank, entry),
                runner_artifact_identity: None,
                instructions: 2,
            }]
        );
    }

    #[test]
    fn block_program_evidence_is_sorted_and_runner_pointer_independent() {
        let first = BankId::new(0x21);
        let second = BankId::new(0x22);
        let artifact = ProgramArtifactIdentity::new([0xA5; 32]);
        let mut forward = BlockProgram::new();
        forward
            .register(
                CodeBank::new(first, VA, vec![0x1111, 0x2222]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(first, first_runner, artifact),
            )
            .unwrap();
        forward
            .register(
                CodeBank::new(second, GuestPc::new(VA.get() + 0x40), vec![0x3333]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(second, second_runner, artifact),
            )
            .unwrap();

        let mut reverse_with_different_runners = BlockProgram::new();
        reverse_with_different_runners
            .register(
                CodeBank::new(second, GuestPc::new(VA.get() + 0x40), vec![0x3333]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(second, first_runner, artifact),
            )
            .unwrap();
        reverse_with_different_runners
            .register(
                CodeBank::new(first, VA, vec![0x1111, 0x2222]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(first, second_runner, artifact),
            )
            .unwrap();

        let snapshot = forward.evidence_snapshot();
        assert_eq!(snapshot, reverse_with_different_runners.evidence_snapshot());
        assert_eq!(
            snapshot.identity.source,
            ProgramIdentitySource::CanonicalBlockProgramSha256
        );
        assert_eq!(
            snapshot
                .banks
                .iter()
                .map(|bank| bank.id)
                .collect::<Vec<_>>(),
            vec![first, second]
        );
    }

    #[test]
    fn block_program_identity_binds_bank_span_and_instruction_families() {
        fn snapshot(id: BankId, start: GuestPc, words: Vec<u32>) -> BlockProgramEvidenceSnapshot {
            let mut program = BlockProgram::new();
            program
                .register(
                    CodeBank::new(id, start, words).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        id,
                        first_runner,
                        ProgramArtifactIdentity::new([0xC3; 32]),
                    ),
                )
                .unwrap();
            program.evidence_snapshot()
        }

        let baseline = snapshot(BankId::new(0x31), VA, vec![0x1111, 0x2222]);
        let changed_bank = snapshot(BankId::new(0x32), VA, vec![0x1111, 0x2222]);
        let changed_span = snapshot(
            BankId::new(0x31),
            GuestPc::new(VA.get() + 4),
            vec![0x1111, 0x2222],
        );
        let changed_word = snapshot(BankId::new(0x31), VA, vec![0x1111, 0x2223]);

        for changed in [&changed_bank, &changed_span, &changed_word] {
            assert_ne!(baseline, *changed);
            assert_ne!(baseline.identity.identity, changed.identity.identity);
        }

        let mut changed_runner_artifact = BlockProgram::new();
        changed_runner_artifact
            .register(
                CodeBank::new(BankId::new(0x31), VA, vec![0x1111, 0x2222]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    BankId::new(0x31),
                    first_runner,
                    ProgramArtifactIdentity::new([0x3C; 32]),
                ),
            )
            .unwrap();
        let changed_runner_artifact = changed_runner_artifact.evidence_snapshot();
        assert_ne!(baseline, changed_runner_artifact);
        assert_ne!(
            baseline.identity.identity,
            changed_runner_artifact.identity.identity
        );
    }

    #[test]
    fn generated_adapter_identity_binds_adapter_runner_and_bank() {
        let baseline = ProgramArtifactIdentity::generated_adapter(
            [0x11; 32],
            [0x22; 32],
            BankId::new(0x33),
            GeneratedAdapterRole::DirectGenerated,
        );
        assert_ne!(
            baseline,
            ProgramArtifactIdentity::generated_adapter(
                [0x10; 32],
                [0x22; 32],
                BankId::new(0x33),
                GeneratedAdapterRole::DirectGenerated,
            )
        );
        assert_ne!(
            baseline,
            ProgramArtifactIdentity::generated_adapter(
                [0x11; 32],
                [0x23; 32],
                BankId::new(0x33),
                GeneratedAdapterRole::DirectGenerated,
            )
        );
        assert_ne!(
            baseline,
            ProgramArtifactIdentity::generated_adapter(
                [0x11; 32],
                [0x22; 32],
                BankId::new(0x34),
                GeneratedAdapterRole::DirectGenerated,
            )
        );
        assert_ne!(
            baseline,
            ProgramArtifactIdentity::generated_adapter(
                [0x22; 32],
                [0x11; 32],
                BankId::new(0x33),
                GeneratedAdapterRole::DirectGenerated,
            )
        );
        assert_ne!(
            baseline,
            ProgramArtifactIdentity::generated_adapter(
                [0x11; 32],
                [0x22; 32],
                BankId::new(0x33),
                GeneratedAdapterRole::EntryContextGate,
            )
        );
    }

    fn mapped_evidence_snapshot(
        bank: BankId,
        spans: &[(u32, u32)],
        mappings: &[(GuestPc, u32, u32, ProgramArtifactIdentity)],
    ) -> BlockProgramEvidenceSnapshot {
        let physical = PhysicalCodeBank::from_spans(
            bank,
            spans
                .iter()
                .map(|&(physical_start, word)| {
                    crate::fetch::PhysicalCodeSpan::new(bank, physical_start, vec![word]).unwrap()
                })
                .collect(),
        )
        .unwrap();
        let mut program = BlockProgram::new();
        program.register_physical_code(physical).unwrap();

        let mut ctx = RecompContext::new();
        for (index, &(entry, physical_address, word, artifact)) in mappings.iter().enumerate() {
            assert_eq!(entry.get() & 0x1fff, 0);
            assert_eq!(physical_address & 0xfff, 0);
            ctx.tlb_entries[index] = crate::runtime::TlbEntryRaw {
                page_mask: 0,
                entry_hi: u64::from(entry.get() & 0xffff_e000),
                entry_lo0: ((physical_address >> 6) & 0x03ff_ffc0) | 0x7,
                entry_lo1: 0,
            };
            let block = MappedAotBlock::new(
                program.physical_code(),
                &ctx,
                bank,
                entry,
                &[word],
                GeneratedBankRunner::new_with_artifact_identity(bank, first_runner, artifact),
            )
            .unwrap();
            program.register_mapped_aot(block).unwrap();
        }
        program.evidence_snapshot()
    }

    #[test]
    fn mapped_block_program_evidence_is_canonical_across_registration_order() {
        let bank = BankId::new(0x51);
        let first_entry = GuestPc::new(0x0040_0000);
        let second_entry = GuestPc::new(0x0040_2000);
        let first_word = 0x2402_0001;
        let second_word = 0x2403_0002;
        let first_artifact = ProgramArtifactIdentity::new([0x11; 32]);
        let second_artifact = ProgramArtifactIdentity::new([0x22; 32]);
        let forward = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, first_word), (0x0020_0000, second_word)],
            &[
                (first_entry, 0x0010_0000, first_word, first_artifact),
                (second_entry, 0x0020_0000, second_word, second_artifact),
            ],
        );
        let reverse = mapped_evidence_snapshot(
            bank,
            &[(0x0020_0000, second_word), (0x0010_0000, first_word)],
            &[
                (second_entry, 0x0020_0000, second_word, second_artifact),
                (first_entry, 0x0010_0000, first_word, first_artifact),
            ],
        );

        assert_eq!(forward, reverse);
        assert_eq!(forward.physical_banks.len(), 1);
        assert_eq!(forward.mapped_aot.len(), 2);
    }

    #[test]
    fn mapped_block_program_identity_binds_physical_and_aot_identity_families() {
        let bank = BankId::new(0x61);
        let entry = GuestPc::new(0x0040_0000);
        let word = 0x2402_0001;
        let artifact = ProgramArtifactIdentity::new([0x33; 32]);
        let baseline = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, word)],
            &[(entry, 0x0010_0000, word, artifact)],
        );
        let changed_bank = mapped_evidence_snapshot(
            BankId::new(0x62),
            &[(0x0010_0000, word)],
            &[(entry, 0x0010_0000, word, artifact)],
        );
        let changed_physical_address = mapped_evidence_snapshot(
            bank,
            &[(0x0020_0000, word)],
            &[(entry, 0x0020_0000, word, artifact)],
        );
        let changed_entry = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, word)],
            &[(GuestPc::new(0x0040_2000), 0x0010_0000, word, artifact)],
        );
        let changed_word = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, word + 1)],
            &[(entry, 0x0010_0000, word + 1, artifact)],
        );
        let changed_artifact = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, word)],
            &[(
                entry,
                0x0010_0000,
                word,
                ProgramArtifactIdentity::new([0x44; 32]),
            )],
        );

        for changed in [
            &changed_bank,
            &changed_physical_address,
            &changed_entry,
            &changed_word,
            &changed_artifact,
        ] {
            assert_ne!(baseline, *changed);
            assert_ne!(baseline.identity.identity, changed.identity.identity);
        }
        assert_eq!(baseline.mapped_aot[0].entry.pc, entry);
        assert_eq!(
            baseline.mapped_aot[0].instructions,
            vec![InstructionWordIdentity::new(bank, 0x0010_0000)]
        );
        assert_eq!(baseline.mapped_aot[0].expected_words, vec![word]);
    }

    fn cross_catalog_mapped_program(compiled_word: u32) -> BlockProgram {
        let bank = BankId::new(0x63);
        let entry = GuestPc::new(0x8010_0000);
        let mut compilation_catalog = PhysicalCodeCatalog::new();
        compilation_catalog
            .register(PhysicalCodeBank::new(bank, 0x0010_0000, vec![compiled_word]).unwrap())
            .unwrap();
        let block = MappedAotBlock::new(
            &compilation_catalog,
            &RecompContext::new(),
            bank,
            entry,
            &[compiled_word],
            GeneratedBankRunner::new_with_artifact_identity(
                bank,
                first_runner,
                ProgramArtifactIdentity::new([0x63; 32]),
            ),
        )
        .unwrap();
        let mut program = BlockProgram::new();
        program
            .register_physical_code(
                PhysicalCodeBank::new(bank, 0x0010_0000, vec![0x2402_0001]).unwrap(),
            )
            .unwrap();
        program.register_mapped_aot(block).unwrap();
        program
    }

    #[test]
    fn mapped_aot_evidence_binds_future_preflight_expected_words() {
        let valid = cross_catalog_mapped_program(0x2402_0001);
        let stale = cross_catalog_mapped_program(0x2402_0002);
        let valid_snapshot = valid.evidence_snapshot();
        let stale_snapshot = stale.evidence_snapshot();
        assert_eq!(valid_snapshot.physical_banks, stale_snapshot.physical_banks);
        assert_eq!(
            valid_snapshot.mapped_aot[0].instructions,
            stale_snapshot.mapped_aot[0].instructions
        );
        assert_ne!(
            valid_snapshot.mapped_aot[0].expected_words,
            stale_snapshot.mapped_aot[0].expected_words
        );
        assert_ne!(
            valid_snapshot.identity.identity,
            stale_snapshot.identity.identity
        );

        let entry = ExecutionKey::new(BankId::new(0x63), GuestPc::new(0x8010_0000));
        let budget = InstructionBudget::new(2).unwrap();
        let mut valid_ctx = RecompContext::new();
        let mut stale_ctx = RecompContext::new();
        let mut valid_storage = [];
        let mut stale_storage = [];
        assert!(!matches!(
            valid
                .run(
                    entry,
                    budget,
                    &mut valid_ctx,
                    &mut Rdram::new(&mut valid_storage),
                )
                .exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::StaleInstructionIdentity { .. },
                ..
            })
        ));
        assert!(matches!(
            stale
                .run(
                    entry,
                    budget,
                    &mut stale_ctx,
                    &mut Rdram::new(&mut stale_storage),
                )
                .exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::StaleInstructionIdentity { .. },
                ..
            })
        ));
    }

    #[test]
    #[should_panic(expected = "stable artifact identity for generated runner")]
    fn block_program_evidence_rejects_unidentified_runner_artifact() {
        let id = BankId::new(0x41);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(id, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new(id, first_runner),
            )
            .unwrap();
        let _ = program.evidence_snapshot();
    }

    #[test]
    fn block_program_rejects_holes_before_invoking_runner() {
        let id = BankId::new(12);
        let sparse = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, VA, vec![1]).unwrap(),
                CodeSpan::new(id, GuestPc::new(VA.get() + 8), vec![2]).unwrap(),
            ],
        )
        .unwrap();
        let mut program = BlockProgram::new();
        program
            .register(sparse, GeneratedBankRunner::new(id, first_runner))
            .unwrap();
        let mut bytes = [];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        let hole = ExecutionKey::new(id, GuestPc::new(VA.get() + 4));
        let run = program.run(hole, InstructionBudget::new(2).unwrap(), &mut ctx, &mut mem);
        assert!(matches!(
            run,
            BlockRun {
                exit: BlockExit::Fault(CpuFault {
                    at,
                    kind: CpuFaultKind::UnmappedPc { .. }
                }),
                instructions: 0,
            } if at == hole
        ));
        assert_eq!(
            ctx.r_u32(2),
            0,
            "runner must not execute for a catalog hole"
        );
        assert!(program.copy_execution_destinations().is_empty());

        let unknown = ExecutionKey::new(BankId::new(0xDEAD), VA);
        assert!(matches!(
            program
                .run(
                    unknown,
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
        assert!(program.copy_execution_destinations().is_empty());
    }

    #[test]
    fn transfers_distinguish_proven_and_runtime_resolved_destinations() {
        let destination = ExecutionKey::new(BankId::new(9), GuestPc::new(0x8000_2000));
        assert_eq!(
            BlockExit::Transfer(destination),
            BlockExit::Transfer(destination)
        );

        let indirect = BlockExit::ResolveTransfer {
            source_bank: BankId::new(1),
            target_pc: GuestPc::new(0x8000_2000),
        };
        assert!(matches!(
            indirect,
            BlockExit::ResolveTransfer {
                source_bank,
                target_pc
            } if source_bank == BankId::new(1) && target_pc == GuestPc::new(0x8000_2000)
        ));
    }

    #[test]
    fn instruction_budget_cannot_split_a_branch_delay_pair() {
        assert_eq!(InstructionBudget::new(0), None);
        let one = InstructionBudget::new(1).unwrap();
        assert_eq!(one.get(), 1);
        assert!(!one.can_fit(0, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS));
        let two = InstructionBudget::new(2).unwrap();
        assert_eq!(two.get(), 2);
        assert!(two.can_fit(0, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS));
        assert!(!two.can_fit(1, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS));
        assert!(!InstructionBudget::new(u32::MAX)
            .unwrap()
            .can_fit(u32::MAX, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS));
    }

    #[test]
    fn malformed_destinations_fault_with_bank_and_pc() {
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(7, &[0])).unwrap();

        let unaligned = ExecutionKey::new(BankId::new(7), GuestPc::new(VA.get() + 2));
        let fault = catalog.resolve(unaligned).unwrap_err();
        assert_eq!(fault, CpuFault::instruction_address_error(unaligned));
        assert!(fault.to_string().contains("bank:0000000000000007"));
        assert!(fault.to_string().contains("0x80001002"));

        let unmapped = ExecutionKey::new(BankId::new(7), GuestPc::new(VA.get() + 4));
        assert!(matches!(
            catalog.resolve(unmapped).unwrap_err().kind,
            CpuFaultKind::UnmappedPc { .. }
        ));

        let unknown = ExecutionKey::new(BankId::new(8), VA);
        assert_eq!(
            catalog.resolve(unknown).unwrap_err().kind,
            CpuFaultKind::UnknownBank
        );
    }

    #[test]
    fn bank_identity_cannot_be_reused_for_new_bytes() {
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(1, &[0x1111])).unwrap();
        assert_eq!(
            catalog.register(bank(1, &[0x2222])),
            Err(BankError::DuplicateId {
                bank: BankId::new(1)
            })
        );
    }

    #[test]
    fn dispatcher_follows_direct_and_resolved_bank_qualified_transfers() {
        let first = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1000));
        let second = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1010));
        let third = ExecutionKey::new(BankId::new(2), GuestPc::new(0x8000_1010));
        let mut runner = |entry: ExecutionKey, _budget: InstructionBudget| match entry {
            key if key == first => BlockRun::new(BlockExit::Transfer(second), 1),
            key if key == second => BlockRun::new(
                BlockExit::ResolveTransfer {
                    source_bank: second.bank,
                    target_pc: second.pc,
                },
                2,
            ),
            key if key == third => BlockRun::new(BlockExit::Yield(third), 1),
            _ => unreachable!("test runner received an unexpected key"),
        };
        let mut resolver = |source_bank: BankId, target_pc: GuestPc| {
            assert_eq!(source_bank, second.bank);
            assert_eq!(target_pc, second.pc);
            Ok(third)
        };

        assert_eq!(
            dispatch_until_boundary(
                first,
                InstructionBudget::new(6).unwrap(),
                &mut runner,
                &mut resolver,
            )
            .unwrap(),
            DispatchRun {
                exit: BlockExit::Yield(third),
                instructions: 4,
                blocks: 3,
            }
        );
    }

    #[test]
    fn dispatcher_reports_an_indivisible_unit_in_the_final_one_instruction_slice() {
        let first = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1000));
        let next = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1004));
        let mut calls = 0;
        let mut runner = |entry, budget: InstructionBudget| {
            calls += 1;
            if entry == next && budget.get() == 1 {
                BlockRun::new(BlockExit::Checkpoint(next), 0)
            } else {
                BlockRun::new(BlockExit::Transfer(next), 1)
            }
        };
        let mut resolver = |_source_bank, _target_pc| unreachable!();

        // A slice that cannot fit an indivisible unit CHECKPOINTS; it does not
        // fail. The unit has not partially executed (instructions == 0), so
        // the caller resumes at the same PC with a full budget and the
        // branch/delay-slot pair retires whole.
        //
        // This previously returned IndivisibleUnitExceedsBudget, which aborted
        // WM2000's certified route at pc=0x800040B8 with one instruction of a
        // 4096 budget left -- a slice boundary reported as a program fault.
        assert_eq!(
            dispatch_until_boundary(
                first,
                InstructionBudget::new(2).unwrap(),
                &mut runner,
                &mut resolver,
            ),
            Ok(DispatchRun {
                exit: BlockExit::Checkpoint(next),
                instructions: 1,
                blocks: 1,
            })
        );
        assert_eq!(calls, 2);
    }

    #[test]
    fn dispatcher_rejects_non_progress_and_budget_violations() {
        let entry = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1000));
        let budget = InstructionBudget::new(2).unwrap();
        let mut resolver = |_source_bank, _target_pc| unreachable!();
        let mut stalled = |_entry, _budget| BlockRun::new(BlockExit::Transfer(entry), 0);
        assert_eq!(
            dispatch_until_boundary(entry, budget, &mut stalled, &mut resolver),
            Err(DispatchError::ContinuingExitWithoutProgress {
                at: entry,
                exit: BlockExit::Transfer(entry),
            })
        );

        let checkpoint = BlockExit::Checkpoint(entry);
        let mut stalled_checkpoint = |_entry, _budget| BlockRun::new(checkpoint, 0);
        assert_eq!(
            dispatch_until_boundary(entry, budget, &mut stalled_checkpoint, &mut resolver),
            Err(DispatchError::ContinuingExitWithoutProgress {
                at: entry,
                exit: checkpoint,
            })
        );

        let mut excessive = |_entry, _budget| BlockRun::new(BlockExit::Yield(entry), 3);
        assert_eq!(
            dispatch_until_boundary(entry, budget, &mut excessive, &mut resolver),
            Err(DispatchError::RunnerExceededBudget {
                at: entry,
                budget,
                actual: 3,
            })
        );
    }

    #[test]
    fn both_dispatchers_reject_zero_progress_executable_write_exits() {
        let bank_id = BankId::new(0x71);
        let budget = InstructionBudget::new(2).unwrap();
        let resume = ExecutionKey::new(bank_id, GuestPc::new(VA.get() + 12));
        let entries_and_exits = [
            (
                ExecutionKey::new(bank_id, VA),
                BlockExit::ExecutableWrite {
                    source_bank: bank_id,
                    resume,
                },
            ),
            (
                ExecutionKey::new(bank_id, GuestPc::new(VA.get() + 4)),
                BlockExit::ExecutableWriteResolveCall {
                    source_bank: bank_id,
                    target_pc: GuestPc::new(0x8000_2000),
                    resume,
                },
            ),
            (
                ExecutionKey::new(bank_id, GuestPc::new(VA.get() + 8)),
                BlockExit::ExecutableWriteFault(CpuFault::instruction_address_error(
                    ExecutionKey::new(bank_id, GuestPc::new(VA.get() + 8)),
                )),
            ),
        ];

        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank_id, VA, vec![0; 3]).unwrap(),
                GeneratedBankRunner::new(bank_id, zero_progress_executable_write_runner),
            )
            .unwrap();
        let mut ctx = RecompContext::new();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);

        for (entry, exit) in entries_and_exits {
            let mut runner = move |_entry, _budget| BlockRun::new(exit, 0);
            let mut resolver = |_source_bank, _target_pc| unreachable!();
            assert_eq!(
                dispatch_until_boundary(entry, budget, &mut runner, &mut resolver),
                Err(DispatchError::ContinuingExitWithoutProgress { at: entry, exit })
            );
            assert_eq!(
                program.dispatch(entry, budget, &mut ctx, &mut mem, &mut resolver),
                Err(DispatchError::ContinuingExitWithoutProgress { at: entry, exit })
            );
        }
    }

    #[test]
    fn executable_write_boundary_preserves_cross_bank_source_lineage() {
        fn changed(_: crate::runtime::GuestWriteEvent) -> crate::runtime::GuestWriteBoundary {
            crate::runtime::GuestWriteBoundary::ExecutableChanged
        }

        let source = BankId::new(0xA);
        let target = ExecutionKey::new(BankId::new(0xC), GuestPc::new(0x8000_4000));
        crate::runtime::set_guest_write_boundary_observer(Some(changed));
        crate::runtime::notify_cpu_instruction_store(0x20, 4);
        assert_eq!(
            finalize_executable_write_exit(source, BlockExit::Transfer(target)),
            BlockExit::ExecutableWrite {
                source_bank: source,
                resume: target,
            }
        );
        assert!(!crate::runtime::take_executable_write_boundary());
        crate::runtime::set_guest_write_boundary_observer(None);
    }

    #[test]
    fn executable_write_special_continuations_escape_dispatch_unresolved() {
        let source = BankId::new(0xA);
        let target = GuestPc::new(0x8000_5000);
        let resume = ExecutionKey::new(source, GuestPc::new(0x8000_1008));
        let call = BlockExit::ExecutableWriteResolveCall {
            source_bank: source,
            target_pc: target,
            resume,
        };
        let mut call_runner = move |_entry, _budget| BlockRun::new(call, 2);
        let mut resolver = |_source_bank, _target_pc| -> Result<ExecutionKey, CpuFault> {
            panic!("executable-write continuation resolved before owner rebuild")
        };
        assert_eq!(
            dispatch_until_boundary(
                ExecutionKey::new(source, VA),
                InstructionBudget::new(4).unwrap(),
                &mut call_runner,
                &mut resolver,
            )
            .unwrap(),
            DispatchRun {
                exit: call,
                instructions: 2,
                blocks: 1,
            }
        );

        let fault = CpuFault::instruction_address_error(ExecutionKey::new(
            source,
            GuestPc::new(0x8000_2002),
        ));
        let mut fault_runner =
            move |_entry, _budget| BlockRun::new(BlockExit::ExecutableWriteFault(fault), 3);
        assert_eq!(
            dispatch_until_boundary(
                ExecutionKey::new(source, VA),
                InstructionBudget::new(4).unwrap(),
                &mut fault_runner,
                &mut resolver,
            )
            .unwrap(),
            DispatchRun {
                exit: BlockExit::ExecutableWriteFault(fault),
                instructions: 3,
                blocks: 1,
            }
        );
    }

    fn source_attestation_fixture(
        binding: CargoGeneratedRunnerSourceBindingV1,
    ) -> Result<CatalogBlockProgramV1, CatalogBlockProgramErrorV1> {
        let artifact = ProgramArtifactIdentity::generated_adapter(
            [0x11; 32],
            [0x33; 32],
            binding.bank,
            GeneratedAdapterRole::DirectGenerated,
        );
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(binding.bank, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    binding.bank,
                    first_runner,
                    artifact,
                ),
            )
            .unwrap();
        CatalogBlockProgramV1::new_with_cargo_generated_runner_source_attestation_v2(
            program,
            ExecutionKey::new(binding.bank, VA),
            InstructionBudget::new(2).unwrap(),
            CargoGeneratedProgramSourceAttestationV2 {
                root_adapter_source_sha256: [0x11; 32],
                shard_cargo_source_tree_sha256: [0x22; 32],
                expected_emitter_source_sha256: [0x44; 32],
                externally_measured_emitter_source_sha256: [0x44; 32],
                expected_runtime_source_sha256: generated_runner_runtime_source_receipt_v1()
                    .source_sha256(),
                runtime_source_receipt: generated_runner_runtime_source_receipt_v1(),
                runners: &[binding],
            },
        )
    }

    fn valid_source_binding() -> CargoGeneratedRunnerSourceBindingV1 {
        CargoGeneratedRunnerSourceBindingV1 {
            bank: BankId::new(0xA701),
            generated_runner_source_sha256: [0x33; 32],
            code_words_sha256: Sha256::digest(0u32.to_be_bytes()).into(),
            vram_start: VA,
            vram_end: GuestPc::new(VA.get() + 4),
            composite_subrunner_count: 32,
            adapter_role: GeneratedAdapterRole::DirectGenerated,
        }
    }

    #[test]
    fn generated_runner_source_attestation_binds_composite_source_program_and_role() {
        let catalog = source_attestation_fixture(valid_source_binding()).unwrap();
        let attestation = catalog
            .generated_runner_source_attestation()
            .expect("source-attested constructor retains its projection");
        assert_eq!(
            attestation.schema(),
            GENERATED_RUNNER_SOURCE_ATTESTATION_SCHEMA_V2
        );
        assert!(attestation.cargo_source_fields_validated());
        assert_eq!(
            attestation.build_receipt(),
            static_execution_build_receipt()
        );

        for malformed in [
            CargoGeneratedRunnerSourceBindingV1 {
                generated_runner_source_sha256: [0x44; 32],
                ..valid_source_binding()
            },
            CargoGeneratedRunnerSourceBindingV1 {
                adapter_role: GeneratedAdapterRole::EntryContextGate,
                ..valid_source_binding()
            },
            CargoGeneratedRunnerSourceBindingV1 {
                vram_end: GuestPc::new(VA.get() + 8),
                ..valid_source_binding()
            },
            CargoGeneratedRunnerSourceBindingV1 {
                code_words_sha256: [0x55; 32],
                ..valid_source_binding()
            },
            CargoGeneratedRunnerSourceBindingV1 {
                composite_subrunner_count: 0,
                ..valid_source_binding()
            },
        ] {
            assert!(source_attestation_fixture(malformed).is_err());
        }
    }

    #[test]
    fn generic_generated_runner_identity_never_claims_source_attestation() {
        let binding = valid_source_binding();
        let artifact = ProgramArtifactIdentity::generated_adapter(
            [0x11; 32],
            binding.generated_runner_source_sha256,
            binding.bank,
            binding.adapter_role,
        );
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(binding.bank, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    binding.bank,
                    first_runner,
                    artifact,
                ),
            )
            .unwrap();
        let catalog = CatalogBlockProgramV1::new(
            program,
            ExecutionKey::new(binding.bank, VA),
            InstructionBudget::new(2).unwrap(),
        )
        .unwrap();
        assert!(catalog.generated_runner_source_attestation().is_none());
    }
