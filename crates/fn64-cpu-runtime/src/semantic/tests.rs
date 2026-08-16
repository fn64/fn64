    use super::*;
    use crate::execution::{CodeBank, CodeSpan};
    use crate::runtime::TlbEntryRaw;

    thread_local! {
        static OBSERVED_READS: std::cell::RefCell<Vec<crate::runtime::GuestReadEvent>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn observe_read(event: crate::runtime::GuestReadEvent) {
        OBSERVED_READS.with(|reads| reads.borrow_mut().push(event));
    }

    const BANK: BankId = BankId::new(0x42);
    const VA: u32 = 0x8000_1000;

    fn catalog_of(words: &[u32]) -> CodeCatalog {
        let bank = CodeBank::new(BANK, GuestPc::new(VA), words.to_vec()).unwrap();
        let mut catalog = CodeCatalog::new();
        catalog.register(bank).unwrap();
        catalog
    }

    fn run(
        catalog: &CodeCatalog,
        pc: u32,
        budget: u32,
        ctx: &mut RecompContext,
    ) -> Result<BlockRun, UnsupportedOp> {
        let mut storage = vec![0u8; 64];
        let mut mem = Rdram::new(&mut storage);
        run_bank(
            catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(pc)),
            InstructionBudget::new(budget).unwrap(),
            ctx,
            &mut mem,
        )
    }

    #[test]
    #[should_panic(expected = "execute_straight_word requires an ordinary instruction")]
    fn straight_word_rejects_control_transfer_as_a_caller_invariant() {
        let mut storage = vec![0u8; 64];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();

        let _ = execute_straight_word(
            BANK,
            VA,
            0x03e0_0008, // jr $ra
            0,
            &mut ctx,
            &mut mem,
        );
    }

    #[test]
    fn unknown_bank_and_unaligned_entry_fault_with_zero_work() {
        // addiu $v0,$zero,1 ; jr $ra ; nop
        let catalog = catalog_of(&[0x2402_0001, 0x03E0_0008, 0x0000_0000]);
        let mut ctx = RecompContext::new();

        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let wrong = run_bank(
            &catalog,
            BANK,
            ExecutionKey::new(BankId::new(0x99), GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();
        assert!(matches!(
            wrong.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnknownBank,
                ..
            })
        ));
        assert_eq!(wrong.instructions, 0);

        let unaligned = run(&catalog, VA + 2, 8, &mut ctx).unwrap();
        assert!(matches!(
            unaligned.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnalignedPc,
                ..
            })
        ));
        assert_eq!(unaligned.instructions, 0);
        assert_eq!(
            ctx.r(2),
            0,
            "faulting entry must not execute any instruction"
        );
    }

    #[test]
    fn unsupported_opcode_is_a_loud_typed_fault_not_a_panic_or_nop() {
        // DMFC0 remains outside this slice: decoded, then a typed unsupported
        // fault naming the op, exactly where the AOT lane traps.
        let dmfc0 = 0x4022_4800;
        let catalog = catalog_of(&[dmfc0, 0x03E0_0008, 0x0000_0000]);
        let mut ctx = RecompContext::new();
        let err = run(&catalog, VA, 8, &mut ctx).unwrap_err();
        assert_eq!(err.at, ExecutionKey::new(BANK, GuestPc::new(VA)));
        assert_eq!(err.instruction, Instruction::Dmfc0 { rt: 2, cop0d: 9 });
    }

    #[test]
    fn reserved_encoding_raises_precise_ri_exception() {
        let reserved = 0x4c00_0000; // reserved primary opcode 0x13
        let catalog = catalog_of(&[reserved, 0x0000_0000]);
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA, 8, &mut ctx).unwrap();
        assert_eq!(run.instructions, 1);
        assert!(matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                at,
                kind: CpuFaultKind::Exception {
                    exception: CpuException::ReservedInstruction,
                    epc,
                    branch_delay: false,
                    ..
                }
            }) if at == ExecutionKey::new(BANK, GuestPc::new(VA)) && epc == GuestPc::new(VA)
        ));
        let BlockExit::Fault(fault) = run.exit else {
            unreachable!()
        };
        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 10);
    }

    #[test]
    fn random_and_tlbwr_follow_interpreter_instruction_order() {
        let words = [
            0x2402_001d, // addiu $v0,$zero,29
            0x4082_3000, // mtc0  $v0,Wired: Random resets to 31, then advances
            0x4200_0006, // tlbwr: samples 30, then advances to 29
            0x4003_0800, // mfc0  $v1,Random: observes 29
            0x03e0_0008, // jr $ra
            0x0000_0000, // nop
        ];
        let catalog = catalog_of(&words);
        let mut ctx = RecompContext::new();
        ctx.set_r(31, 0x8000_9000);
        ctx.cop0_entry_hi = 0x1234_500a;
        ctx.cop0_entry_lo0 = 0x46;
        ctx.cop0_entry_lo1 = 0x86;
        ctx.cop0_page_mask = 0x6000;

        let result = run(&catalog, VA, words.len() as u32, &mut ctx).unwrap();
        assert_eq!(result.instructions, words.len() as u32);
        assert_eq!(ctx.r_u32(3), 29);
        assert_eq!(ctx.tlb_entries[30].entry_hi, 0x1234_500a);
        assert_eq!(ctx.tlb_entries[30].entry_lo0, 0x46);
        assert_eq!(ctx.tlb_entries[30].entry_lo1, 0x86);
        assert_eq!(ctx.tlb_entries[30].page_mask, 0x6000);
        assert_eq!(ctx.read_cop0(1), 29);
    }

    #[test]
    fn annulled_likely_slot_consumes_the_runners_second_random_unit() {
        let words = [
            0x5002_0001, // beql $zero,$v0,+1: not taken when v0=1
            0x2403_0077, // addiu $v1,$zero,0x77: annulled
            0x4004_0800, // mfc0 $a0,Random
        ];
        let catalog = catalog_of(&words);
        let mut ctx = RecompContext::new();
        ctx.set_r(2, 1);

        let branch = run(&catalog, VA, 3, &mut ctx).unwrap();
        assert_eq!(branch.instructions, 2);
        assert_eq!(
            branch.exit,
            BlockExit::Transfer(ExecutionKey::new(BANK, GuestPc::new(VA + 8)))
        );
        assert_eq!(
            ctx.r_u32(3),
            0,
            "likely delay instruction must stay annulled"
        );
        let sample = run(&catalog, VA + 8, 2, &mut ctx).unwrap();
        assert_eq!(sample.instructions, 1);
        assert_eq!(
            ctx.r_u32(4),
            29,
            "branch plus annulled charged unit advance Random twice"
        );
        assert_eq!(ctx.read_cop0(1), 28, "MFC0 retires after sampling Random");
    }

    #[test]
    fn modeled_cop0_and_indexed_tlb_management_execute_in_the_interpreter() {
        let mtc0 = |rt: u32, rd: u32| 0x4080_0000 | (rt << 16) | (rd << 11);
        let mfc0 = |rt: u32, rd: u32| 0x4000_0000 | (rt << 16) | (rd << 11);
        let words = [
            mtc0(2, 10), // EntryHi
            mtc0(3, 2),  // EntryLo0
            mtc0(4, 3),  // EntryLo1
            mtc0(5, 5),  // PageMask
            mtc0(6, 0),  // Index
            0x4200_0002, // TLBWI
            mtc0(7, 10), // probe EntryHi
            0x4200_0008, // TLBP
            mfc0(8, 0),  // matched Index
            0x4200_0001, // TLBR
            mfc0(9, 10), // reloaded EntryHi
            0x03e0_0008, // jr $ra
            0,
        ];
        let catalog = catalog_of(&words);
        let mut ctx = RecompContext::new();
        ctx.set_r(2, 0x1234_400a);
        ctx.set_r(3, 0x0000_0046);
        ctx.set_r(4, 0x0000_0086);
        ctx.set_r(5, 0x0000_6000);
        ctx.set_r(6, 7);
        ctx.set_r(7, 0x1234_200a);

        let result = run(&catalog, VA, words.len() as u32, &mut ctx).unwrap();
        assert_eq!(result.instructions, words.len() as u32);
        assert_eq!(ctx.r_u32(8), 7);
        assert_eq!(ctx.r_u32(9), 0x1234_400a);
        assert_eq!(ctx.cop0_page_mask, 0x0000_6000);
        assert_eq!(ctx.cop0_entry_lo0, 0x0000_0046);
        assert_eq!(ctx.cop0_entry_lo1, 0x0000_0086);
    }

    #[test]
    fn mid_block_mfc0_count_sees_live_phase_and_interior_delta_without_double_counting() {
        // Three MFC0 $9 reads inside ONE block, at retired-instruction
        // offsets 0, 5, and 10 (four NOPs between each), then `jr $ra`. Gap 2:
        // Count is normally synchronized only at block/checkpoint boundaries
        // (the executor owns it, `RecompContext::synchronize_cop0_timing`
        // writes it once at block entry); an in-block MFC0 $9 must instead
        // see Count advanced by (retired instructions since entry) / 2, at
        // the same half-CPU-rate the executor's `advance_time` uses.
        let mfc0_count = |rt: u32| 0x4000_4800 | (rt << 16); // mfc0 $rt, $9
        let words = [
            mfc0_count(8), // $t0 <- Count @ retired_before = 0
            0,
            0,
            0,
            0,             // 4 nops
            mfc0_count(9), // $t1 <- Count @ retired_before = 5
            0,
            0,
            0,
            0,              // 4 nops
            mfc0_count(10), // $t2 <- Count @ retired_before = 10
            0x03E0_0008,    // jr $ra
            0,              // nop (delay)
        ];
        let catalog = catalog_of(&words);
        let mut ctx = RecompContext::new();
        ctx.set_r(31, 0x8000_9000);
        // Simulate the block-entry boundary sync: the live executor's
        // authoritative Count at the moment this block was dispatched.
        const ENTRY_COUNT: u32 = 1_000;
        ctx.synchronize_cop0_timing(ENTRY_COUNT, 0, 0);

        let result = run(&catalog, VA, words.len() as u32, &mut ctx).unwrap();
        assert_eq!(result.instructions, words.len() as u32);

        // Interior reads: base + retired_before/2, matching the executor's
        // half-CPU-rate (integer-divided) advance.
        assert_eq!(
            ctx.r_u32(8),
            ENTRY_COUNT,
            "first mfc0 (retired_before=0) sees the pristine entry Count"
        );
        assert_eq!(
            ctx.r_u32(9),
            ENTRY_COUNT + 5 / 2,
            "second mfc0 (retired_before=5) sees +2, not the stale entry value"
        );
        assert_eq!(
            ctx.r_u32(10),
            ENTRY_COUNT + 10 / 2,
            "third mfc0 (retired_before=10) sees +5"
        );

        // The boundary-authority contract: `ctx.cop0_count` itself (the field
        // the NEXT block-entry sync would overwrite from the executor, and
        // that this test uses to emulate the executor's own post-block
        // advance) was NEVER mutated by any interior read above.
        assert_eq!(
            ctx.cop0_count, ENTRY_COUNT,
            "interior MFC0 reads must not write ctx.cop0_count \u{2014} \
             only the boundary sync may, or the executor's authoritative \
             advance would double-count these same retired instructions"
        );

        // Emulate exactly what the live executor does after this block: it
        // independently computes `retired_total / 2` from the SAME
        // `result.instructions` this block returned, and that is the whole
        // and only advance applied at the boundary. Applying it once here
        // must land on entry + total/2 — not entry + (sum of the three
        // interior deltas), which would be the double-count this design
        // avoids.
        let boundary_advanced = ENTRY_COUNT + result.instructions / 2;
        assert_eq!(
            boundary_advanced,
            ENTRY_COUNT + 13 / 2,
            "sanity: 13 total retired instructions advance Count by 6"
        );
        let sum_of_interior_deltas: u32 = 5 / 2 + 10 / 2; // deliberately re-summing the
                                                          // three per-read deltas (the
                                                          // first is +0) to prove the
                                                          // boundary does NOT do this
        assert_ne!(
            boundary_advanced,
            ENTRY_COUNT + sum_of_interior_deltas,
            "the boundary must not re-sum the three interior deltas on top \
             of its own advance"
        );

        // An odd phase at entry completes a Count interval after the first
        // retired instruction. This state was previously lost at the live
        // executor -> block-context boundary.
        let mut odd_phase_ctx = RecompContext::new();
        odd_phase_ctx.set_r(31, 0x8000_9000);
        odd_phase_ctx.synchronize_cop0_timing(ENTRY_COUNT, 1, 0);
        let odd_phase_result = run(&catalog, VA, words.len() as u32, &mut odd_phase_ctx).unwrap();
        assert_eq!(odd_phase_result.instructions, words.len() as u32);
        assert_eq!(odd_phase_ctx.r_u32(8), ENTRY_COUNT);
        assert_eq!(odd_phase_ctx.r_u32(9), ENTRY_COUNT + (1 + 5) / 2);
        assert_eq!(odd_phase_ctx.r_u32(10), ENTRY_COUNT + (1 + 10) / 2);
        assert_eq!(odd_phase_ctx.cop0_count, ENTRY_COUNT);
    }

    /// ERET (COP0 function 0x18): `eret`.
    const ERET: u32 = 0x4200_0018;

    #[test]
    fn eret_under_erl_prefers_error_epc_clears_erl_and_clears_llbit() {
        const STATUS_EXL: u32 = 1 << 1;
        const STATUS_ERL: u32 = 1 << 2;

        let catalog = catalog_of(&[ERET]);
        let mut ctx = RecompContext::new();
        ctx.cop0_status = STATUS_EXL | STATUS_ERL;
        ctx.cop0_epc = 0x8000_1000;
        ctx.cop0_error_epc = 0xBFC0_0200;
        ctx.set_ll_reservation(0x8000_0040, 4);

        let result = run(&catalog, VA, 8, &mut ctx).unwrap();

        assert_eq!(
            result.exit,
            BlockExit::ResolveTransfer {
                source_bank: BANK,
                target_pc: GuestPc::new(0xBFC0_0200),
            },
            "ErrorEPC/ERL takes precedence over EPC/EXL, exactly as emit_bank_eret"
        );
        assert_eq!(result.instructions, 1, "eret has no delay slot");
        assert_eq!(ctx.cop0_status & STATUS_ERL, 0, "ERL must clear");
        assert_ne!(
            ctx.cop0_status & STATUS_EXL,
            0,
            "EXL is untouched under ERL precedence"
        );
        assert!(
            !ctx.take_ll_reservation(0x8000_0040, 4),
            "eret must clear LLbit"
        );
    }

    #[test]
    fn eret_without_erl_falls_back_to_epc_and_clears_exl() {
        const STATUS_EXL: u32 = 1 << 1;
        const STATUS_ERL: u32 = 1 << 2;

        let catalog = catalog_of(&[ERET]);
        let mut ctx = RecompContext::new();
        ctx.cop0_status = STATUS_EXL;
        ctx.cop0_epc = 0x8000_2004;
        ctx.cop0_error_epc = 0xBFC0_0200;
        ctx.set_ll_reservation(0x8000_0040, 4);

        let result = run(&catalog, VA, 8, &mut ctx).unwrap();

        assert_eq!(
            result.exit,
            BlockExit::ResolveTransfer {
                source_bank: BANK,
                target_pc: GuestPc::new(0x8000_2004),
            },
            "without ERL, eret returns to EPC"
        );
        assert_eq!(result.instructions, 1);
        assert_eq!(ctx.cop0_status & STATUS_EXL, 0, "EXL must clear");
        assert_eq!(ctx.cop0_status & STATUS_ERL, 0, "ERL was already clear");
        assert!(
            !ctx.take_ll_reservation(0x8000_0040, 4),
            "eret must clear LLbit"
        );
    }

    #[test]
    fn eret_matches_the_block_lanes_resolve_transfer_shape_even_for_an_in_bank_target() {
        // The AOT lane (`emit_bank_eret`) always emits an unconditional
        // `BlockExit::ResolveTransfer`, never a proven in-bank `Transfer`,
        // because the target is a runtime CP0 value. Pick an EPC that
        // happens to land inside this very bank and confirm the interpreter
        // still resolves rather than proving.
        let catalog = catalog_of(&[ERET, 0x0000_0000]);
        let mut ctx = RecompContext::new();
        ctx.cop0_status = 0;
        ctx.cop0_epc = VA; // in-bank, but must still be ResolveTransfer

        let result = run(&catalog, VA, 8, &mut ctx).unwrap();
        assert_eq!(
            result.exit,
            BlockExit::ResolveTransfer {
                source_bank: BANK,
                target_pc: GuestPc::new(VA),
            }
        );
        assert_eq!(result.instructions, 1);
    }

    #[test]
    fn memory_fault_reports_effective_address_and_excludes_the_faulting_op() {
        // lui $t0,0x8000 ; sw $v0,0x40($t0) (offset 0x40 outside 16-byte rdram)
        let catalog = catalog_of(&[0x3C08_8000, 0xAD02_0040, 0x03E0_0008, 0x0000_0000]);
        let mut ctx = RecompContext::new();
        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let run = run_bank(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();
        match run.exit {
            BlockExit::Fault(CpuFault {
                at,
                kind: CpuFaultKind::MemoryFault { addr },
            }) => {
                assert_eq!(at, ExecutionKey::new(BANK, GuestPc::new(VA + 4)));
                assert_eq!(addr, 0xFFFF_FFFF_8000_0040);
            }
            other => panic!("expected typed MemoryFault, got {other:?}"),
        }
        // Only the LUI retired; the faulting SW is excluded.
        assert_eq!(run.instructions, 1);
    }

    #[test]
    fn budget_checkpoints_before_a_branch_delay_pair_without_splitting_it() {
        // Two straight ops, then a branch: a 2-instruction budget must stop at
        // the branch's PC with the pair uncharged.
        let catalog = catalog_of(&[
            0x2402_0001, // addiu $v0,$zero,1
            0x2442_0002, // addiu $v0,$v0,2
            0x1042_0001, // beq $v0,$v0,+1
            0x2404_0007, // addiu $a0,$zero,7 (delay)
            0x03E0_0008, // jr $ra
            0x0000_0000, // nop
        ]);
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA, 2, &mut ctx).unwrap();
        assert_eq!(run.instructions, 2);
        assert_eq!(
            run.exit,
            BlockExit::Checkpoint(ExecutionKey::new(BANK, GuestPc::new(VA + 8)))
        );
        assert_eq!(
            ctx.r(2),
            3,
            "the two straight ops retired before the checkpoint"
        );
        assert_eq!(ctx.r(4), 0, "the delay slot must not have run");
    }

    #[test]
    fn one_instruction_budget_checkpoints_before_an_initial_branch_delay_pair() {
        let catalog = catalog_of(&[
            0x1000_0001, // beq $zero,$zero,+1
            0x2404_0007, // addiu $a0,$zero,7 (delay)
            0x2405_0009,
        ]);
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA, 1, &mut ctx).unwrap();
        assert_eq!(
            run,
            BlockRun::new(
                BlockExit::Checkpoint(ExecutionKey::new(BANK, GuestPc::new(VA))),
                0,
            )
        );
        assert_eq!(ctx.r(4), 0, "the delay slot must not have run");
        assert_eq!(ctx.r(5), 0, "the branch target must not have run");
    }

    #[test]
    fn jr_snapshots_target_before_a_delay_slot_that_overwrites_the_source() {
        // jr $t0 ; addiu $t0,$zero,0x1234 (delay overwrites $t0)
        let catalog = catalog_of(&[0x0100_0008, 0x2408_1234]);
        let mut ctx = RecompContext::new();
        ctx.set_r(8, 0x8000_2000);
        let run = run(&catalog, VA, 8, &mut ctx).unwrap();
        assert_eq!(ctx.r_u32(8), 0x1234, "the delay slot ran");
        assert_eq!(
            run.exit,
            BlockExit::ResolveTransfer {
                source_bank: BANK,
                target_pc: GuestPc::new(0x8000_2000),
            },
            "the transfer used the pre-delay snapshot"
        );
        assert_eq!(run.instructions, 2);
    }

    #[test]
    fn falling_out_of_the_bank_hands_the_virtual_pc_to_the_mapping_layer() {
        // A single straight op whose fallthrough is outside the admitted bank.
        let catalog = catalog_of(&[0x2402_0001]);
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA, 8, &mut ctx).unwrap();
        assert_eq!(
            run.exit,
            BlockExit::ResolveTransfer {
                source_bank: BANK,
                target_pc: GuestPc::new(VA + 4),
            }
        );
        assert_eq!(run.instructions, 1);
    }

    #[test]
    fn arbitrary_pc_execution_reports_ordered_rdram_read_dependencies() {
        let catalog = catalog_of(&[
            0x3c08_8000, // lui $t0,0x8000
            0x8d02_0000, // lw $v0,0($t0)
            0x8903_0005, // lwl $v1,5($t0)
            0xc104_0008, // ll $a0,8($t0)
            0xc500_000c, // lwc1 $f0,12($t0)
            0xd502_0010, // ldc1 $f2,16($t0)
            0x03e0_0008, // jr $ra
            0x0000_0000, // nop
        ]);
        let mut storage = vec![0u8; 32];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        ctx.cop0_status = 1 << 29;
        OBSERVED_READS.with(|reads| reads.borrow_mut().clear());
        let previous = crate::runtime::set_read_observer(Some(observe_read));
        let run = run_bank(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(16).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();
        crate::runtime::set_read_observer(previous);

        assert!(matches!(run.exit, BlockExit::ResolveTransfer { .. }));
        assert_eq!(
            OBSERVED_READS.with(|reads| reads.borrow().clone()),
            vec![
                crate::runtime::GuestReadEvent {
                    physical_offset: 0,
                    len: 4,
                },
                crate::runtime::GuestReadEvent {
                    physical_offset: 4,
                    len: 4,
                },
                crate::runtime::GuestReadEvent {
                    physical_offset: 8,
                    len: 4,
                },
                crate::runtime::GuestReadEvent {
                    physical_offset: 12,
                    len: 4,
                },
                crate::runtime::GuestReadEvent {
                    physical_offset: 16,
                    len: 8,
                },
            ]
        );
    }

    #[test]
    fn a_data_hole_between_spans_is_never_executed() {
        // Two disjoint spans with a hole at VA+4; entering the hole faults typed.
        let bank = CodeBank::from_spans(
            BANK,
            vec![
                CodeSpan::new(BANK, GuestPc::new(VA), vec![0x2402_0001]).unwrap(),
                CodeSpan::new(BANK, GuestPc::new(VA + 8), vec![0x2403_0002]).unwrap(),
            ],
        )
        .unwrap();
        let mut catalog = CodeCatalog::new();
        catalog.register(bank).unwrap();
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA + 4, 8, &mut ctx).unwrap();
        assert!(matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnmappedPc { .. },
                ..
            })
        ));
        assert_eq!(run.instructions, 0);
    }

    #[test]
    fn self_loop_runs_its_delay_slot_then_yields() {
        // beq $zero,$zero,self ; addiu $a0,$zero,7 (delay)
        let catalog = catalog_of(&[0x1000_FFFF, 0x2404_0007]);
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA, 8, &mut ctx).unwrap();
        assert_eq!(
            run.exit,
            BlockExit::Yield(ExecutionKey::new(BANK, GuestPc::new(VA)))
        );
        assert_eq!(run.instructions, 2);
        assert_eq!(
            ctx.r(4),
            7,
            "the self-loop delay slot runs before the yield"
        );
    }

    // -- MMIO seam (interpreter side) --------------------------------------
    //
    // A minimal in-crate mock port standing in for the runtime's real device
    // model: it owns ONE modeled register value and claims exactly ONE KSEG1
    // window (`0xFFFF_FFFF_A460_0000..A460_1000`, the PI block). Everything
    // outside that window is `NotMmio`, so it exercises the load-bearing
    // property that an MMIO window does not make arbitrary addresses succeed.
    // The runtime-side integration test (`fn64-runtime/tests/`) proves the SAME
    // seam against the crate's actual `DeviceFabric`/`MmioSpace` state.
    struct MockPiPort {
        /// The one modeled register's value (a PI_STATUS-like word).
        reg: u32,
        /// Reads/writes observed, for asserting the port was actually hit.
        reads: u32,
        writes: u32,
    }

    // The single register this mock models: PI_STATUS at KSEG1 0xA460_0010,
    // sign-extended to the 64-bit effective address the guest computes.
    const PI_STATUS_VADDR: u64 = 0xFFFF_FFFF_A460_0010;
    const PI_WINDOW_LO: u64 = 0xFFFF_FFFF_A460_0000;
    const PI_WINDOW_HI: u64 = 0xFFFF_FFFF_A460_1000;

    impl MmioPort for MockPiPort {
        fn read_w(&mut self, vaddr: u64) -> MmioOutcome<u32> {
            if !(PI_WINDOW_LO..PI_WINDOW_HI).contains(&vaddr) {
                return MmioOutcome::NotMmio;
            }
            if vaddr == PI_STATUS_VADDR {
                self.reads += 1;
                MmioOutcome::Handled(self.reg)
            } else {
                // In-window but unmodeled register: a loud typed fault, never a
                // silent 0 (mirrors MmioSpace::read_w's panic-to-fault stance).
                MmioOutcome::Fault { addr: vaddr }
            }
        }
        fn write_w(&mut self, vaddr: u64, value: u32) -> MmioOutcome<()> {
            if !(PI_WINDOW_LO..PI_WINDOW_HI).contains(&vaddr) {
                return MmioOutcome::NotMmio;
            }
            if vaddr == PI_STATUS_VADDR {
                self.writes += 1;
                self.reg = value;
                MmioOutcome::Handled(())
            } else {
                MmioOutcome::Fault { addr: vaddr }
            }
        }
    }

    struct MockCartridgePort {
        physical_base: u32,
        words: [u32; 2],
        readable_len: Option<usize>,
        offered_reads: u32,
        offered_stores: u32,
    }

    impl MockCartridgePort {
        fn offset(&self, address: AlignedDirectWordAddress) -> Option<usize> {
            let physical = address.get() as u32 & 0x1fff_ffff;
            let offset = physical.checked_sub(self.physical_base)?;
            (offset < 8).then_some(offset as usize)
        }
    }

    impl CartridgeWordPort for MockCartridgePort {
        fn read_w(&mut self, address: AlignedDirectWordAddress) -> CartridgeReadOutcome {
            self.offered_reads += 1;
            let Some(offset) = self.offset(address) else {
                return CartridgeReadOutcome::NotCartridge;
            };
            match self.readable_len {
                Some(len) if offset.checked_add(4).is_some_and(|end| end <= len) => {
                    CartridgeReadOutcome::Handled(self.words[offset / 4])
                }
                Some(_) | None => CartridgeReadOutcome::Fault,
            }
        }

        fn classify_store_w(&mut self, address: AlignedDirectWordAddress) -> CartridgeStoreOutcome {
            self.offered_stores += 1;
            if self.offset(address).is_some() {
                CartridgeStoreOutcome::ReadOnlyFault
            } else {
                CartridgeStoreOutcome::NotCartridge
            }
        }
    }

    #[derive(Default)]
    struct BroadCartridgePort {
        offered_reads: u32,
        offered_stores: u32,
    }

    impl CartridgeWordPort for BroadCartridgePort {
        fn read_w(&mut self, _address: AlignedDirectWordAddress) -> CartridgeReadOutcome {
            self.offered_reads += 1;
            CartridgeReadOutcome::Handled(0xdead_beef)
        }

        fn classify_store_w(
            &mut self,
            _address: AlignedDirectWordAddress,
        ) -> CartridgeStoreOutcome {
            self.offered_stores += 1;
            CartridgeStoreOutcome::ReadOnlyFault
        }
    }

    fn no_mmio_port() -> MockPiPort {
        MockPiPort {
            reg: 0,
            reads: 0,
            writes: 0,
        }
    }

    #[test]
    fn interpreted_cartridge_loads_accept_canonical_kseg0_and_kseg1() {
        let catalog = catalog_of(&[
            0x3c08_9000, // lui $t0,0x9000 (cached cartridge alias)
            0x8d02_0000, // lw $v0,0($t0)
            0x3c08_b000, // lui $t0,0xb000 (uncached cartridge alias)
            0x8d03_0004, // lw $v1,4($t0)
            0x03e0_0008,
            0,
        ]);
        let mut cartridge = MockCartridgePort {
            physical_base: 0x1000_0000,
            words: [0x1020_3040, 0xaabb_ccdd],
            readable_len: Some(8),
            offered_reads: 0,
            offered_stores: 0,
        };
        let mut mmio = no_mmio_port();
        let mut storage = [0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let run = run_bank_with_memory_port(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
            &mut MemoryPort::new(&mut mmio, &mut cartridge),
        )
        .unwrap();
        assert!(matches!(run.exit, BlockExit::ResolveTransfer { .. }));
        assert_eq!(ctx.r(2), 0x1020_3040);
        assert_eq!(ctx.r(3), 0xffff_ffff_aabb_ccdd);
        assert_eq!(cartridge.offered_reads, 2);
        assert_eq!(mmio.reads, 0);
    }

    #[test]
    fn direct_word_token_rejects_noncanonical_and_delegates_window_boundaries() {
        assert_eq!(
            AlignedDirectWordAddress::from_translated(0x0000_0001_b000_0000),
            None
        );
        assert_eq!(
            AlignedDirectWordAddress::from_translated(0xffff_ffff_b000_0001),
            None
        );
        let mut cartridge = MockCartridgePort {
            physical_base: 0x1000_0000,
            words: [0; 2],
            readable_len: Some(8),
            offered_reads: 0,
            offered_stores: 0,
        };
        let before = AlignedDirectWordAddress::from_translated(0xffff_ffff_afff_fffc).unwrap();
        let after = AlignedDirectWordAddress::from_translated(0xffff_ffff_b000_0008).unwrap();
        assert_eq!(cartridge.read_w(before), CartridgeReadOutcome::NotCartridge);
        assert_eq!(cartridge.read_w(after), CartridgeReadOutcome::NotCartridge);
    }

    #[test]
    fn noncanonical_and_unaligned_loads_fault_before_cartridge_classification() {
        let catalog = catalog_of(&[0x8d02_0000, 0x03e0_0008, 0]);
        for address in [0x0000_0001_b000_0000, 0xffff_ffff_b000_0001] {
            let mut cartridge = MockCartridgePort {
                physical_base: 0x1000_0000,
                words: [0; 2],
                readable_len: Some(8),
                offered_reads: 0,
                offered_stores: 0,
            };
            let mut mmio = no_mmio_port();
            let mut storage = [0u8; 16];
            let mut mem = Rdram::new(&mut storage);
            let mut ctx = RecompContext::new();
            ctx.set_r(8, address);
            let run = run_bank_with_memory_port(
                &catalog,
                BANK,
                ExecutionKey::new(BANK, GuestPc::new(VA)),
                InstructionBudget::new(4).unwrap(),
                &mut ctx,
                &mut mem,
                &mut MemoryPort::new(&mut mmio, &mut cartridge),
            )
            .unwrap();
            assert!(matches!(run.exit, BlockExit::Fault(_)));
            assert_eq!(cartridge.offered_reads, 0);
        }
    }

    #[test]
    fn mapped_cartridge_physical_address_is_not_direct_cartridge_access() {
        let catalog = catalog_of(&[0x8d02_0000, 0x03e0_0008, 0]);
        let mut cartridge = MockCartridgePort {
            physical_base: 0x1000_0000,
            words: [0xfeed_face, 0],
            readable_len: Some(8),
            offered_reads: 0,
            offered_stores: 0,
        };
        let mut mmio = no_mmio_port();
        let mut storage = [0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        ctx.set_r(8, 0x0040_0000);
        ctx.tlb_entries[0] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0x0040_0000,
            entry_lo0: (0x10000 << 6) | 0b111,
            entry_lo1: 0b111,
        };
        let run = run_bank_with_memory_port(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(4).unwrap(),
            &mut ctx,
            &mut mem,
            &mut MemoryPort::new(&mut mmio, &mut cartridge),
        )
        .unwrap();
        assert!(matches!(run.exit, BlockExit::Fault(_)));
        assert_eq!(cartridge.offered_reads, 0);
    }

    #[test]
    fn absent_and_out_of_bounds_cartridge_reads_fault_typed() {
        let catalog = catalog_of(&[0x3c08_b000, 0x8d02_0004, 0x03e0_0008, 0]);
        for readable_len in [None, Some(4)] {
            let mut cartridge = MockCartridgePort {
                physical_base: 0x1000_0000,
                words: [0; 2],
                readable_len,
                offered_reads: 0,
                offered_stores: 0,
            };
            let mut mmio = no_mmio_port();
            let mut storage = [0u8; 16];
            let mut mem = Rdram::new(&mut storage);
            let mut ctx = RecompContext::new();
            let run = run_bank_with_memory_port(
                &catalog,
                BANK,
                ExecutionKey::new(BANK, GuestPc::new(VA)),
                InstructionBudget::new(8).unwrap(),
                &mut ctx,
                &mut mem,
                &mut MemoryPort::new(&mut mmio, &mut cartridge),
            )
            .unwrap();
            assert!(matches!(
                run.exit,
                BlockExit::Fault(CpuFault {
                    kind: CpuFaultKind::MemoryFault { .. },
                    ..
                })
            ));
            assert_eq!(ctx.r(2), 0);
            assert_eq!(mmio.reads, 0);
        }
    }

    #[test]
    fn read_only_cartridge_store_faults_before_backing_changes() {
        let mut cartridge = MockCartridgePort {
            physical_base: 0x1000_0000,
            words: [0; 2],
            readable_len: Some(8),
            offered_reads: 0,
            offered_stores: 0,
        };
        let mut mmio = no_mmio_port();
        let mut storage = [0x5au8; 8];
        let before = storage;
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        ctx.set_r32(2, 0x1122_3344);
        ctx.set_r32(8, 0xb000_0000u32 as i32);
        let catalog = catalog_of(&[0xad02_0000, 0x03e0_0008, 0]);
        let run = run_bank_with_memory_port(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(4).unwrap(),
            &mut ctx,
            &mut mem,
            &mut MemoryPort::new(&mut mmio, &mut cartridge),
        )
        .unwrap();
        assert!(matches!(run.exit, BlockExit::Fault(_)));
        assert_eq!(cartridge.offered_stores, 1);
        assert_eq!(storage, before);
    }

    #[test]
    fn broad_cartridge_cannot_shadow_backed_rdram() {
        let catalog = catalog_of(&[0x8d02_0000, 0x03e0_0008, 0]);
        let mut cartridge = BroadCartridgePort::default();
        let mut mmio = no_mmio_port();
        let mut storage = [0u8; 8];
        let mut mem = Rdram::new(&mut storage);
        mem.store_w(0xffff_ffff_8000_0000, 0x1234_5678);
        let mut ctx = RecompContext::new();
        ctx.set_r32(8, 0x8000_0000u32 as i32);
        run_bank_with_memory_port(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(4).unwrap(),
            &mut ctx,
            &mut mem,
            &mut MemoryPort::new(&mut mmio, &mut cartridge),
        )
        .unwrap();
        assert_eq!(ctx.r(2), 0x1234_5678);
        assert_eq!(cartridge.offered_reads, 0);
    }

    #[test]
    fn default_entrypoint_does_not_install_cartridge_authority() {
        let catalog = catalog_of(&[0x3c08_b000, 0x8d02_0000, 0x03e0_0008, 0]);
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA, 8, &mut ctx).unwrap();
        assert!(matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::MemoryFault { .. },
                ..
            })
        ));
        assert_eq!(ctx.r(2), 0);
    }

    #[test]
    fn broad_cartridge_cannot_shadow_handled_mmio() {
        let catalog = catalog_of(&[0x3c08_a460, 0x8d02_0010, 0x03e0_0008, 0]);
        let mut cartridge = BroadCartridgePort::default();
        let mut mmio = MockPiPort {
            reg: 0x1234_5678,
            reads: 0,
            writes: 0,
        };
        let mut storage = [0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        run_bank_with_memory_port(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
            &mut MemoryPort::new(&mut mmio, &mut cartridge),
        )
        .unwrap();
        assert_eq!(cartridge.offered_reads, 0);
        assert_eq!(mmio.reads, 1);
        assert_eq!(ctx.r(2), 0x1234_5678);
    }

    #[test]
    fn interpreted_mmio_load_gets_the_modeled_register_value() {
        // lui $t0,0xA460 ; lw $v0,0x10($t0) ; jr $ra ; nop
        // The lw's effective address is the modeled PI_STATUS; the interpreter
        // must return the port's value, not read RDRAM (which would fault).
        let catalog = catalog_of(&[0x3C08_A460, 0x8D02_0010, 0x03E0_0008, 0x0000_0000]);
        let mut port = MockPiPort {
            reg: 0xDEAD_BEEF,
            reads: 0,
            writes: 0,
        };
        let mut storage = vec![0u8; 64];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        OBSERVED_READS.with(|reads| reads.borrow_mut().clear());
        let previous = crate::runtime::set_read_observer(Some(observe_read));
        let run = run_bank_with_mmio(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
            &mut port,
        )
        .unwrap();
        crate::runtime::set_read_observer(previous);
        assert_eq!(port.reads, 1, "the modeled register was read once");
        // Word register value sign-extends into the GPR exactly as a real LW.
        assert_eq!(ctx.r(2), 0xFFFF_FFFF_DEAD_BEEF);
        assert!(matches!(run.exit, BlockExit::ResolveTransfer { .. }));
        assert!(OBSERVED_READS.with(|reads| reads.borrow().is_empty()));
    }

    #[test]
    fn interpreted_mmio_store_updates_the_modeled_register_state() {
        // lui $t0,0xA460 ; ori $v0,$zero,0 ; sw $v0,0x10($t0) ; jr $ra ; nop
        // A store of 0 to PI_STATUS updates the modeled state through the port.
        let catalog = catalog_of(&[
            0x3C08_A460, // lui $t0,0xA460
            0x3402_0000, // ori $v0,$zero,0
            0xAD02_0010, // sw $v0,0x10($t0)
            0x03E0_0008, // jr $ra
            0x0000_0000, // nop
        ]);
        let mut port = MockPiPort {
            reg: 0b11, // busy+error set, as after a DMA start
            reads: 0,
            writes: 0,
        };
        let mut storage = vec![0u8; 64];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        run_bank_with_mmio(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
            &mut port,
        )
        .unwrap();
        assert_eq!(port.writes, 1, "the modeled register was written once");
        assert_eq!(port.reg, 0, "the store updated modeled device state");
    }

    #[test]
    fn a_non_mmio_out_of_rdram_load_still_faults_typed_with_a_port_present() {
        // The load-bearing safety property: an MMIO window present must NOT make
        // an arbitrary out-of-RDRAM address succeed. lui $t0,0x8000 ; lw
        // $v0,0x40($t0) reads 0x8000_0040 — outside the 16-byte rdram AND
        // outside the port's PI window — so it must be a typed MemoryFault, the
        // same as with no port at all.
        let catalog = catalog_of(&[0x3C08_8000, 0x8D02_0040, 0x03E0_0008, 0x0000_0000]);
        let mut port = MockPiPort {
            reg: 0xDEAD_BEEF,
            reads: 0,
            writes: 0,
        };
        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let run = run_bank_with_mmio(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
            &mut port,
        )
        .unwrap();
        match run.exit {
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::MemoryFault { addr },
                ..
            }) => assert_eq!(addr, 0xFFFF_FFFF_8000_0040),
            other => panic!("expected typed MemoryFault, got {other:?}"),
        }
        assert_eq!(port.reads, 0, "the port was not consulted-as-handled");
        assert_eq!(ctx.r(2), 0, "the faulting load wrote no register");
    }

    #[test]
    fn an_in_window_unmodeled_register_is_a_typed_fault_not_a_nop() {
        // A load in the PI window but at an unmodeled offset (0x14) is a typed
        // MemoryFault (the port's Fault outcome), never a silent success.
        // lui $t0,0xA460 ; lw $v0,0x14($t0)
        let catalog = catalog_of(&[0x3C08_A460, 0x8D02_0014, 0x03E0_0008, 0x0000_0000]);
        let mut port = MockPiPort {
            reg: 0,
            reads: 0,
            writes: 0,
        };
        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let run = run_bank_with_mmio(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
            &mut port,
        )
        .unwrap();
        match run.exit {
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::MemoryFault { addr },
                ..
            }) => assert_eq!(addr, 0xFFFF_FFFF_A460_0014),
            other => panic!("expected typed MemoryFault for unmodeled register, got {other:?}"),
        }
    }

    #[test]
    fn run_bank_default_no_mmio_still_faults_an_mmio_address() {
        // Without a port (plain run_bank), an MMIO-window load is just an
        // out-of-RDRAM MemoryFault — proving the seam is opt-in and the default
        // path is byte-identical to before it existed.
        let catalog = catalog_of(&[0x3C08_A460, 0x8D02_0010, 0x03E0_0008, 0x0000_0000]);
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA, 8, &mut ctx).unwrap();
        assert!(matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::MemoryFault { .. },
                ..
            })
        ));
    }

    /// Raised exception for a one-instruction bank, or `None` when the
    /// instruction retired without faulting. `jr $ra; nop` terminates the bank.
    fn trap_exception_of(word: u32, regs: &[(u8, u64)]) -> Option<CpuException> {
        let catalog = catalog_of(&[word, 0x03E0_0008, 0x0000_0000]);
        let mut ctx = RecompContext::new();
        for &(reg, value) in regs {
            ctx.set_r(reg, value);
        }
        let run = run(&catalog, VA, 8, &mut ctx).unwrap();
        match run.exit {
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::Exception { exception, .. },
                ..
            }) => Some(exception),
            _ => None,
        }
    }

    #[test]
    fn interpreted_break_and_syscall_raise_their_architectural_exceptions() {
        // break 0 / syscall 0 — the mid-function traps that previously left the
        // interpreter with no arm at all (a typed UnsupportedOp dead-end).
        assert_eq!(
            trap_exception_of(0x0000_000D, &[]),
            Some(CpuException::Breakpoint)
        );
        assert_eq!(
            trap_exception_of(0x0000_000C, &[]),
            Some(CpuException::Syscall)
        );
    }

    #[test]
    fn interpreted_conditional_trap_respects_its_condition() {
        // teq $t0,$t0 always holds -> Trap; tne $t0,$t0 never holds -> the
        // instruction retires as an ordinary no-op rather than faulting.
        assert_eq!(
            trap_exception_of(0x0108_0034, &[]),
            Some(CpuException::Trap)
        );
        assert_eq!(trap_exception_of(0x0108_0036, &[]), None);
    }

    #[test]
    fn interpreted_conditional_trap_uses_the_architectural_comparison_width() {
        // $t0 = -1, $t1 = 1. Signed tlt takes the trap (-1 < 1); unsigned tltu
        // does not (0xFFFF_FFFF_FFFF_FFFF > 1). Comparing at the wrong width
        // silently inverts both, so this pins the exact bug class.
        let regs = [(8u8, u64::MAX), (9u8, 1u64)];
        assert_eq!(
            trap_exception_of(0x0109_0032, &regs),
            Some(CpuException::Trap),
            "signed tlt must trap for -1 < 1"
        );
        assert_eq!(
            trap_exception_of(0x0109_0033, &regs),
            None,
            "unsigned tltu must not trap: -1 reads as u64::MAX"
        );
    }

    fn executable_boundary(
        event: crate::runtime::GuestWriteEvent,
    ) -> crate::runtime::GuestWriteBoundary {
        let (start, len) = event.range();
        if start < 0x24 && start.saturating_add(len) > 0x20 {
            crate::runtime::GuestWriteBoundary::ExecutableChanged
        } else {
            crate::runtime::GuestWriteBoundary::Continue
        }
    }

    #[test]
    fn interpreted_delay_slot_store_stops_at_selected_target_without_splitting_pair() {
        // beq $zero,$zero,+2 ; sw $t0,0($a0) ; stale ; target
        let catalog = catalog_of(&[0x1000_0002, 0xac88_0000, 0x2402_0001, 0x2403_0002]);
        let mut storage = vec![0u8; 0x100];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        ctx.set_r(4, 0xffff_ffff_8000_0020);
        ctx.set_r(8, 0x1122_3344);
        crate::runtime::set_guest_write_boundary_observer(Some(executable_boundary));

        let run = run_bank(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();
        assert_eq!(run.instructions, 2);
        assert_eq!(ctx.r(2), 0, "fallthrough sentinel must not execute");
        assert_eq!(ctx.r(3), 0, "selected-target sentinel must not execute");
        assert_eq!(mem.load_w(0xffff_ffff_8000_0020) as u32, 0x1122_3344);
        assert_eq!(
            run.exit,
            BlockExit::ExecutableWrite {
                source_bank: BANK,
                resume: ExecutionKey::new(BANK, GuestPc::new(VA + 12)),
            }
        );
        assert!(!crate::runtime::take_executable_write_boundary());
        crate::runtime::set_guest_write_boundary_observer(None);
    }

    #[test]
    fn annulled_likely_slot_does_not_fabricate_an_executable_write() {
        // bnel $zero,$zero,+2 is not taken, so its store slot is annulled.
        let catalog = catalog_of(&[0x5400_0002, 0xac88_0000, 0x2402_0001, 0x2403_0002]);
        let mut storage = vec![0u8; 0x100];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        ctx.set_r(4, 0xffff_ffff_8000_0020);
        ctx.set_r(8, 0x1122_3344);
        crate::runtime::set_guest_write_boundary_observer(Some(executable_boundary));

        let run = run_bank(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();
        assert_eq!(run.instructions, 2);
        assert!(matches!(run.exit, BlockExit::Transfer(_)));
        assert_eq!(mem.load_w(0xffff_ffff_8000_0020), 0);
        assert!(!crate::runtime::take_executable_write_boundary());
        crate::runtime::set_guest_write_boundary_observer(None);
    }

    #[test]
    fn faulting_and_failed_conditional_stores_request_no_boundary() {
        let sw_catalog = catalog_of(&[0xac88_0000, 0x2402_0001]);
        let mut storage = vec![0u8; 0x100];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        ctx.set_r(4, 0xffff_ffff_8000_0021);
        ctx.set_r(8, 0x1122_3344);
        crate::runtime::set_guest_write_boundary_observer(Some(executable_boundary));
        let fault = run_bank(
            &sw_catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();
        assert!(matches!(fault.exit, BlockExit::Fault(_)));
        assert!(!crate::runtime::take_executable_write_boundary());

        let sc_catalog = catalog_of(&[0xe088_0000, 0x2402_0001, 0x03e0_0008, 0]);
        ctx.set_r(4, 0xffff_ffff_8000_0020);
        ctx.set_r(8, 0x5566_7788);
        ctx.set_r(31, 0x8000_9000);
        let failed = run_bank(
            &sc_catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();
        assert_eq!(ctx.r(8), 0);
        assert_eq!(ctx.r(2), 1, "failed SC continues to the sentinel");
        assert!(!matches!(failed.exit, BlockExit::ExecutableWrite { .. }));
        assert!(!crate::runtime::take_executable_write_boundary());
        crate::runtime::set_guest_write_boundary_observer(None);
    }

    #[test]
    fn successful_conditional_store_stops_before_the_next_instruction() {
        let catalog = catalog_of(&[0xe088_0000, 0x2402_0001, 0x03e0_0008, 0]);
        let mut storage = vec![0u8; 0x100];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let addr = 0xffff_ffff_8000_0020;
        ctx.set_r(4, addr);
        ctx.set_r(8, 0x99aa_bbcc);
        ctx.set_ll_reservation(addr, 4);
        crate::runtime::set_guest_write_boundary_observer(Some(executable_boundary));
        let run = run_bank(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();
        assert_eq!(run.instructions, 1);
        assert_eq!(ctx.r(8), 1);
        assert_eq!(ctx.r(2), 0);
        assert_eq!(
            run.exit,
            BlockExit::ExecutableWrite {
                source_bank: BANK,
                resume: ExecutionKey::new(BANK, GuestPc::new(VA + 4)),
            }
        );
        crate::runtime::set_guest_write_boundary_observer(None);
    }

    #[test]
    fn delay_slot_executable_store_preserves_target_fetch_budget_and_fault() {
        // jr $t1 ; sw $t0,0($a0). The pair retires first. An exactly exhausted
        // budget checkpoints the selected target; one more unit admits the
        // counted fetch attempt and its AdEL without entering a handler before
        // the executable owner rebuilds.
        let catalog = catalog_of(&[0x0120_0008, 0xac88_0000]);
        let target = ExecutionKey::new(BANK, GuestPc::new(0x8000_2002));
        for budget in [2, 3] {
            let mut storage = vec![0u8; 0x100];
            let mut mem = Rdram::new(&mut storage);
            let mut ctx = RecompContext::new();
            ctx.set_r(4, 0xffff_ffff_8000_0020);
            ctx.set_r(8, 0x1122_3344);
            ctx.set_r(9, u64::from(target.pc.get()));
            crate::runtime::set_guest_write_boundary_observer(Some(executable_boundary));
            let run = run_bank(
                &catalog,
                BANK,
                ExecutionKey::new(BANK, GuestPc::new(VA)),
                InstructionBudget::new(budget).unwrap(),
                &mut ctx,
                &mut mem,
            )
            .unwrap();
            if budget == 2 {
                assert_eq!(run, BlockRun::new(BlockExit::Checkpoint(target), 2));
            } else {
                assert_eq!(run.instructions, 3);
                assert_eq!(
                    run.exit,
                    BlockExit::ExecutableWriteFault(CpuFault::instruction_address_error(target))
                );
            }
            assert!(!crate::runtime::take_executable_write_boundary());
        }
        crate::runtime::set_guest_write_boundary_observer(None);
    }

    #[test]
    fn enabled_cop1_register_and_control_moves_are_bit_exact() {
        let catalog = catalog_of(&[
            0x4482_1800, // mtc1 $v0,$f3
            0x4404_1800, // mfc1 $a0,$f3
            0x44A3_2000, // dmtc1 $v1,$f4
            0x4425_2000, // dmfc1 $a1,$f4
            0x44C6_F800, // ctc1 $a2,$fcr31
            0x4447_F800, // cfc1 $a3,$fcr31
            0x4448_0000, // cfc1 $t0,$fcr0
        ]);
        let mut ctx = RecompContext::new();
        ctx.cop0_status = 1 << 29;
        ctx.set_d_bits(2, 0x1122_3344_5566_7788);
        ctx.set_r(2, 0x8123_4567);
        ctx.set_r(3, 0x89AB_CDEF_0123_4567);
        ctx.set_r(6, 0x0180_007F);

        let run = run(&catalog, VA, 8, &mut ctx).unwrap();
        assert_eq!(run.instructions, 7);
        assert_eq!(ctx.d_bits(2), 0x8123_4567_5566_7788);
        assert_eq!(ctx.r(4), 0xFFFF_FFFF_8123_4567);
        assert_eq!(ctx.d_bits(4), 0x89AB_CDEF_0123_4567);
        assert_eq!(ctx.r(5), 0x89AB_CDEF_0123_4567);
        assert_eq!(ctx.read_fcr(31), 0x0180_007F);
        assert_eq!(ctx.r(7), 0x0180_007F);
        assert_eq!(ctx.r(8), 0x0000_0B00);
    }

    #[test]
    fn ctc1_writes_fcsr_then_raises_precise_fpe_with_delay_context() {
        for (words, expected_at, expected_epc, branch_delay, instructions, value) in [
            (
                vec![0x44C2_F800, 0x2404_0007],
                VA,
                VA,
                false,
                1,
                0x0001_0804,
            ),
            (
                vec![0x1000_0001, 0x44C2_F800, 0],
                VA + 4,
                VA,
                true,
                2,
                1 << 17,
            ),
        ] {
            let catalog = catalog_of(&words);
            let mut ctx = RecompContext::new();
            ctx.cop0_status = 1 << 29;
            ctx.set_r(2, value);
            let run = run(&catalog, VA, 4, &mut ctx).unwrap();
            assert!(matches!(
                run.exit,
                BlockExit::Fault(CpuFault {
                    at,
                    kind: CpuFaultKind::Exception {
                        exception: CpuException::FloatingPoint,
                        epc,
                        branch_delay: got_bd,
                        instruction_code: 0,
                        bad_vaddr: None,
                        coprocessor: None,
                    },
                }) if at == ExecutionKey::new(BANK, GuestPc::new(expected_at))
                    && epc == GuestPc::new(expected_epc)
                    && got_bd == branch_delay
            ));
            assert_eq!(run.instructions, instructions);
            assert_eq!(ctx.read_fcr(31), value as u32);
            assert_eq!(ctx.r(4), 0, "post-CTC1 sentinel executed");
        }
    }

    #[test]
    fn disabled_cop1_moves_fault_before_fpr_or_fcsr_mutation() {
        for word in [
            0x4482_1800, // mtc1 $v0,$f3
            0x44A2_2000, // dmtc1 $v0,$f4
            0x44C2_F800, // ctc1 $v0,$fcr31
        ] {
            let catalog = catalog_of(&[word]);
            let mut ctx = RecompContext::new();
            ctx.set_r(2, u64::MAX);
            ctx.set_d_bits(2, 0x1122_3344_5566_7788);
            ctx.set_d_bits(4, 0x99AA_BBCC_DDEE_FF00);
            ctx.write_fcr(31, 3);

            let run = run(&catalog, VA, 2, &mut ctx).unwrap();
            assert!(matches!(
                run.exit,
                BlockExit::Fault(CpuFault {
                    kind: CpuFaultKind::Exception {
                        exception: CpuException::CoprocessorUnusable,
                        coprocessor: Some(1),
                        ..
                    },
                    ..
                })
            ));
            assert_eq!(run.instructions, 1);
            assert_eq!(ctx.d_bits(2), 0x1122_3344_5566_7788);
            assert_eq!(ctx.d_bits(4), 0x99AA_BBCC_DDEE_FF00);
            assert_eq!(ctx.read_fcr(31), 3);
        }
    }

    #[test]
    fn every_decoded_cop0_family_checks_authority_before_shape_or_effect() {
        for word in [
            0x4002_4800, // MFC0
            0x4022_3800, // DMFC0 unsupported register shape
            0x4084_6000, // MTC0 Status
            0x40A2_3800, // DMTC0 unsupported register shape
            0x4100_0001, // BC0F
            0x4101_0001, // BC0T
            0x4102_0001, // BC0FL
            0x4103_0001, // BC0TL
            0x4200_0001, // TLBR
            0x4200_0002, // TLBWI
            0x4200_0006, // TLBWR
            0x4200_0008, // TLBP
            0x4200_0018, // ERET
        ] {
            let instruction = decode(word);
            let words = if instruction.has_delay_slot() {
                vec![word, 0x2404_0007, 0]
            } else {
                vec![word]
            };
            let catalog = catalog_of(&words);
            let mut ctx = RecompContext::new();
            ctx.cop0_status = 2 << 3;
            ctx.set_r(2, 0x1122_3344_5566_7788);
            ctx.cop0_epc = 0x8000_2000;
            ctx.cop0_error_epc = 0x8000_3000;
            ctx.cop0_index = 7;
            ctx.cop0_page_mask = 0x6000;
            ctx.cop0_entry_hi = 0x1234_500A;
            ctx.set_ll_reservation(0x8000_0040, 4);
            let random = ctx.read_cop0(1);

            let run = run(&catalog, VA, 4, &mut ctx).unwrap();
            assert!(matches!(
                run.exit,
                BlockExit::Fault(CpuFault {
                    at,
                    kind: CpuFaultKind::Exception {
                        exception: CpuException::CoprocessorUnusable,
                        epc,
                        branch_delay: false,
                        coprocessor: Some(0),
                        ..
                    },
                }) if at == ExecutionKey::new(BANK, GuestPc::new(VA))
                    && epc == GuestPc::new(VA)
            ));
            assert_eq!(run.instructions, 1, "instruction={instruction:?}");
            assert_eq!(ctx.cop0_status, 2 << 3);
            assert_eq!(ctx.r(2), 0x1122_3344_5566_7788);
            assert_eq!(ctx.r(4), 0, "COP0 branch delay executed");
            assert_eq!(ctx.cop0_epc, 0x8000_2000);
            assert_eq!(ctx.cop0_error_epc, 0x8000_3000);
            assert_eq!(ctx.cop0_index, 7);
            assert_eq!(ctx.cop0_page_mask, 0x6000);
            assert_eq!(ctx.cop0_entry_hi, 0x1234_500A);
            assert_eq!(ctx.read_cop0(1), random);
            assert!(ctx.take_ll_reservation(0x8000_0040, 4));
        }
    }

    #[test]
    fn enabled_cop1_single_and_double_compares_commit_exact_predicates() {
        let single = catalog_of(&[0x4602_003C]); // c.lt.s $f0,$f2
        let mut ctx = RecompContext::new();
        ctx.cop0_status = 1 << 29;
        ctx.set_f_s(0, 1.0);
        ctx.set_f_s(2, 2.0);
        let first = run(&single, VA, 2, &mut ctx).unwrap();
        assert!(ctx.fpu_cond);
        assert_eq!(first.instructions, 1);

        let double = catalog_of(&[0x4622_0032]); // c.eq.d $f0,$f2
        ctx.set_f_d(0, 4.0);
        ctx.set_f_d(2, 4.0);
        let second = run(&double, VA, 2, &mut ctx).unwrap();
        assert!(ctx.fpu_cond);
        assert_eq!(second.instructions, 1);
        assert_eq!(ctx.read_fcr(31) & (0x3F << 12), 0);
    }

    #[test]
    fn enabled_compare_invalid_suppresses_condition_with_precise_delay_context() {
        const CAUSE_V: u32 = 1 << 16;
        const ENABLE_V: u32 = 1 << 11;
        const FLAG_V: u32 = 1 << 6;
        const FLAG_I: u32 = 1 << 2;

        for (words, expected_at, branch_delay, instructions, double) in [
            (vec![0x4602_0032, 0x2404_0007], VA, false, 1, false),
            (vec![0x1000_0001, 0x4622_0032, 0], VA + 4, true, 2, true),
        ] {
            let catalog = catalog_of(&words);
            let mut ctx = RecompContext::new();
            ctx.cop0_status = 1 << 29;
            ctx.write_fcr(31, (1 << 23) | ENABLE_V | FLAG_I | 3);
            if double {
                ctx.set_d_bits(0, 0x7FF8_0000_0000_0001);
                ctx.set_f_d(2, 1.0);
            } else {
                ctx.set_f_bits(0, 0x7FC0_0001);
                ctx.set_f_s(2, 1.0);
            }

            let run = run(&catalog, VA, 4, &mut ctx).unwrap();
            assert!(matches!(
                run.exit,
                BlockExit::Fault(CpuFault {
                    at,
                    kind: CpuFaultKind::Exception {
                        exception: CpuException::FloatingPoint,
                        epc,
                        branch_delay: got_bd,
                        ..
                    },
                }) if at == ExecutionKey::new(BANK, GuestPc::new(expected_at))
                    && epc == GuestPc::new(VA)
                    && got_bd == branch_delay
            ));
            assert_eq!(run.instructions, instructions);
            assert!(ctx.fpu_cond, "enabled Invalid committed condition");
            assert_eq!(
                ctx.read_fcr(31),
                (1 << 23) | CAUSE_V | ENABLE_V | FLAG_I | 3
            );
            assert_eq!(ctx.read_fcr(31) & FLAG_V, 0);
            assert_eq!(ctx.r(4), 0, "post-compare sentinel executed");
        }
    }

    #[test]
    fn float_to_fixed_commits_only_a_typed_success() {
        const CAUSE_I: u32 = 1 << 12;
        const ENABLE_I: u32 = 1 << 7;
        const FLAG_I: u32 = 1 << 2;

        let success = catalog_of(&[0x4600_0124]); // cvt.w.s $f4,$f0
        let mut ctx = RecompContext::new();
        ctx.cop0_status = 1 << 29;
        ctx.set_f_s(0, 1.5);
        let success_run = run(&success, VA, 2, &mut ctx).unwrap();
        assert_eq!(success_run.instructions, 1);
        assert_eq!(ctx.f_bits(4), 2);
        assert_eq!(ctx.read_fcr(31), CAUSE_I | FLAG_I);

        ctx.set_f_s(0, 1.5);
        ctx.set_f_bits(4, 0xA5A5_5A5A);
        ctx.write_fcr(31, ENABLE_I);
        let enabled_run = run(&success, VA, 2, &mut ctx).unwrap();
        assert!(matches!(
            enabled_run.exit,
            BlockExit::Fault(CpuFault {
                at,
                kind: CpuFaultKind::Exception {
                    exception: CpuException::FloatingPoint,
                    epc,
                    branch_delay: false,
                    ..
                },
            }) if at == ExecutionKey::new(BANK, GuestPc::new(VA))
                && epc == GuestPc::new(VA)
        ));
        assert_eq!(enabled_run.instructions, 1);
        assert_eq!(ctx.f_bits(4), 0xA5A5_5A5A);
        assert_eq!(ctx.read_fcr(31), CAUSE_I | ENABLE_I);

        let delay = catalog_of(&[
            0x1000_0001, // beq $zero,$zero,+1
            0x4620_0125, // cvt.l.d $f4,$f0 -- QNaN => E
            0,
        ]);
        ctx.set_d_bits(0, 0x7FF0_0000_0000_0001);
        ctx.set_d_bits(4, 0x1122_3344_5566_7788);
        ctx.write_fcr(31, FLAG_I);
        let run = run(&delay, VA, 4, &mut ctx).unwrap();
        assert!(matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                at,
                kind: CpuFaultKind::Exception {
                    exception: CpuException::FloatingPoint,
                    epc,
                    branch_delay: true,
                    ..
                },
            }) if at == ExecutionKey::new(BANK, GuestPc::new(VA + 4))
                && epc == GuestPc::new(VA)
        ));
        assert_eq!(run.instructions, 2);
        assert_eq!(ctx.d_bits(4), 0x1122_3344_5566_7788);
        assert_eq!(ctx.read_fcr(31), (1 << 17) | FLAG_I);
    }

    #[test]
    fn fixed_to_float_commits_only_a_typed_success() {
        const CAUSE_I: u32 = 1 << 12;
        const CAUSE_E: u32 = 1 << 17;
        const ENABLE_I: u32 = 1 << 7;
        const FLAG_I: u32 = 1 << 2;

        let conversion = catalog_of(&[0x46A0_1121]); // cvt.d.l $f4,$f2
        let mut ctx = RecompContext::new();
        ctx.cop0_status = 1 << 29;
        ctx.set_d_bits(2, 0x0020_0000_0000_0001);
        ctx.write_fcr(31, 2);
        let success = run(&conversion, VA, 2, &mut ctx).unwrap();
        assert_eq!(success.instructions, 1);
        assert_eq!(ctx.d_bits(4), 0x4340_0000_0000_0001);
        assert_eq!(ctx.read_fcr(31), 2 | CAUSE_I | FLAG_I);

        ctx.set_d_bits(4, 0x1122_3344_5566_7788);
        ctx.write_fcr(31, ENABLE_I);
        let enabled = run(&conversion, VA, 2, &mut ctx).unwrap();
        assert!(matches!(
            enabled.exit,
            BlockExit::Fault(CpuFault {
                at,
                kind: CpuFaultKind::Exception {
                    exception: CpuException::FloatingPoint,
                    epc,
                    branch_delay: false,
                    ..
                },
            }) if at == ExecutionKey::new(BANK, GuestPc::new(VA))
                && epc == GuestPc::new(VA)
        ));
        assert_eq!(enabled.instructions, 1);
        assert_eq!(ctx.d_bits(4), 0x1122_3344_5566_7788);
        assert_eq!(ctx.read_fcr(31), CAUSE_I | ENABLE_I);

        ctx.set_d_bits(2, 1 << 55);
        ctx.set_d_bits(4, 0x8877_6655_4433_2211);
        ctx.write_fcr(31, FLAG_I);
        let unimplemented = run(&conversion, VA, 2, &mut ctx).unwrap();
        assert!(matches!(
            unimplemented.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::Exception {
                    exception: CpuException::FloatingPoint,
                    ..
                },
                ..
            })
        ));
        assert_eq!(ctx.d_bits(4), 0x8877_6655_4433_2211);
        assert_eq!(ctx.read_fcr(31), CAUSE_E | FLAG_I);
    }

    #[test]
    fn all_float_to_fixed_opcodes_map_and_commit_the_destination_width() {
        let cases = [
            ("round.l.s", 0x4600_0008, true, false),
            ("trunc.l.s", 0x4600_0009, true, false),
            ("ceil.l.s", 0x4600_000A, true, false),
            ("floor.l.s", 0x4600_000B, true, false),
            ("round.w.s", 0x4600_000C, false, false),
            ("trunc.w.s", 0x4600_000D, false, false),
            ("ceil.w.s", 0x4600_000E, false, false),
            ("floor.w.s", 0x4600_000F, false, false),
            ("cvt.w.s", 0x4600_0024, false, false),
            ("cvt.l.s", 0x4600_0025, true, false),
            ("round.l.d", 0x4620_0008, true, true),
            ("trunc.l.d", 0x4620_0009, true, true),
            ("ceil.l.d", 0x4620_000A, true, true),
            ("floor.l.d", 0x4620_000B, true, true),
            ("round.w.d", 0x4620_000C, false, true),
            ("trunc.w.d", 0x4620_000D, false, true),
            ("ceil.w.d", 0x4620_000E, false, true),
            ("floor.w.d", 0x4620_000F, false, true),
            ("cvt.w.d", 0x4620_0024, false, true),
            ("cvt.l.d", 0x4620_0025, true, true),
        ];

        for (name, encoding, long, double) in cases {
            let catalog = catalog_of(&[encoding | (4 << 6)]);
            let mut ctx = RecompContext::new();
            ctx.cop0_status = 1 << 29;
            if double {
                ctx.set_f_d(0, 1.0);
            } else {
                ctx.set_f_s(0, 1.0);
            }
            ctx.set_d_bits(4, 0xA5A5_5A5A_DEAD_BEEF);

            let result = run(&catalog, VA, 2, &mut ctx).unwrap();
            assert_eq!(result.instructions, 1, "{name}");
            if long {
                assert_eq!(ctx.d_bits(4), 1, "{name} must commit all 64 bits");
            } else {
                assert_eq!(
                    ctx.d_bits(4),
                    0xA5A5_5A5A_0000_0001,
                    "{name} must preserve the paired high word"
                );
            }
        }
    }
