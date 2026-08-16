//! The arbitrary-PC bank-runner gate, moved whole from bank_runner.rs.
//! Its embedded rustc-block template strings are deliberately not
//! deduplicated against the harness: the generated program must stay
//! byte-stable independent of harness refactors.
use super::*;

#[test]
fn emitted_bank_runner_compiles_and_executes_from_arbitrary_pcs() {
    let emitted = emit_bank_runner(&BankInput {
        name: "run_test_bank",
        bank: BankId::new(0xA5),
        vram: BASE,
        words: &WORDS,
    });
    let leaf_words = [0x2402_002A, 0x03E0_0008, 0x0000_0000];
    let emitted_leaf_bank = emit_bank_runner(&BankInput {
        name: "run_leaf_bank",
        bank: BankId::new(0xB6),
        vram: 0x8000_3000,
        words: &leaf_words,
    });
    let emitted_leaf_function =
        fn64_cpu_runtime_codegen::emit_function(&fn64_cpu_runtime_codegen::FuncInput {
            name: "run_leaf_function",
            vram: 0x8000_3000,
            words: &leaf_words,
        });
    let host_call_words = [0x0C00_0800, 0x2404_0007, 0x2402_0009];
    let emitted_host_call_bank = emit_bank_runner_with_host_calls(
        &BankInput {
            name: "run_host_call_bank",
            bank: BankId::new(0xBD),
            vram: 0x8000_9000,
            words: &host_call_words,
        },
        &[0x8000_2000],
    );
    let dynamic_call_words = [0x0320_F809, 0x2404_0008, 0x2402_000A];
    let emitted_dynamic_call_bank = emit_bank_runner(&BankInput {
        name: "run_dynamic_call_bank",
        bank: BankId::new(0xBE),
        vram: 0x8000_A000,
        words: &dynamic_call_words,
    });
    let sparse_first = [
        0x0800_1408, // j     0x80005020
        0x2404_0009, // addiu $a0,$zero,9 (delay)
    ];
    let sparse_second = [
        0x0100_0008, // jr    $t0
        0x2408_1234, // addiu $t0,$zero,0x1234 (delay)
    ];
    let sparse_blocks = [
        BankBlockInput {
            vram: 0x8000_5000,
            words: &sparse_first,
        },
        BankBlockInput {
            vram: 0x8000_5020,
            words: &sparse_second,
        },
    ];
    let emitted_sparse_bank = emit_sparse_bank_runner(&SparseBankInput {
        name: "run_sparse_bank",
        bank: BankId::new(0xC7),
        blocks: &sparse_blocks,
    });
    let exception_words = [
        (0x123 << 6) | 0x0C, // syscall 0x123
        0x1000_0001,         // beq $zero,$zero,+1
        (7 << 6) | 0x0D,     // break 7 (delay slot)
        0x0000_0000,
        (4 << 21) | (5 << 16) | (0x55 << 6) | 0x34, // teq $a0,$a1,0x55
        0x0000_0000,
    ];
    let emitted_exception_bank = emit_bank_runner(&BankInput {
        name: "run_exception_bank",
        bank: BankId::new(0xD8),
        vram: 0x8000_6000,
        words: &exception_words,
    });
    let overflow_words = [0x2042_0001, 0x0000_0000]; // addi $v0,$v0,1; nop
    let emitted_overflow_bank = emit_bank_runner(&BankInput {
        name: "run_overflow_bank",
        bank: BankId::new(0xE9),
        vram: 0x8000_7000,
        words: &overflow_words,
    });
    let address_words = [
        0x8C82_0001, // lw  $v0,1($a0): AdEL
        0x1000_0001, // beq $zero,$zero,+1
        0xAC82_0002, // sw  $v0,2($a0): AdES in the delay slot
        0x0000_0000,
    ];
    let emitted_address_bank = emit_bank_runner(&BankInput {
        name: "run_address_bank",
        bank: BankId::new(0xEA),
        vram: 0x8000_B000,
        words: &address_words,
    });
    let cop1_words = [
        0x4402_0000, // mfc1  $v0,$f0
        0x1000_0001, // beq   $zero,$zero,+1
        0x4402_0000, // mfc1  $v0,$f0 (delay)
        0x0000_0000,
        0x4501_0001, // bc1t  +1
        0x2404_0007, // addiu $a0,$zero,7 (delay)
        0x0000_0000,
        0xC482_0001, // lwc1  $f2,1($a0)
        0x0000_0000,
    ];
    let emitted_cop1_bank = emit_bank_runner(&BankInput {
        name: "run_cop1_bank",
        bank: BankId::new(0xEB),
        vram: 0x8000_D000,
        words: &cop1_words,
    });
    // An enabled FP exception (FCSR Enable.V set + sqrt(-1) -> Invalid) must
    // vector to ExcCode 15 WITHOUT writing the destination register. The first
    // bank raises it straight-line; the second raises it in a branch delay slot
    // so Cause.BD and EPC (the branch, not the slot) can be checked.
    let fp_trap_words = [
        0x4600_1004, // sqrt.s $f0,$f2
        0x0000_0000, // nop
    ];
    let emitted_fp_trap_bank = emit_bank_runner(&BankInput {
        name: "run_fp_trap_bank",
        bank: BankId::new(0xEC),
        vram: 0x8000_E000,
        words: &fp_trap_words,
    });
    let fp_delay_words = [
        0x1000_0001, // beq   $zero,$zero,+1
        0x4600_1004, // sqrt.s $f0,$f2 (delay slot)
        0x0000_0000, // nop
        0x0000_0000, // nop
    ];
    let emitted_fp_delay_bank = emit_bank_runner(&BankInput {
        name: "run_fp_delay_bank",
        bank: BankId::new(0xED),
        vram: 0x8000_F000,
        words: &fp_delay_words,
    });
    let exception_handler_words = [
        0x4008_7000, // mfc0  $t0,$14 (EPC)
        0x2508_0004, // addiu $t0,$t0,4
        0x4088_7000, // mtc0  $t0,$14 (EPC)
        0x4200_0018, // eret
    ];
    let emitted_exception_handler = emit_bank_runner(&BankInput {
        name: "run_exception_handler",
        bank: BankId::new(0xFA),
        vram: 0x8000_0180,
        words: &exception_handler_words,
    });
    let fetch_handler_words = [
        0x3C08_8000, // lui   $t0,0x8000
        0x3508_1004, // ori   $t0,$t0,0x1004
        0x4088_7000, // mtc0  $t0,$14 (EPC)
        0x4200_0018, // eret
    ];
    let emitted_fetch_handler = emit_bank_runner(&BankInput {
        name: "run_fetch_handler",
        bank: BankId::new(0xF9),
        vram: 0x8000_0180,
        words: &fetch_handler_words,
    });
    let eret_words = [0x4200_0018];
    let emitted_eret_bank = emit_bank_runner(&BankInput {
        name: "run_eret_bank",
        bank: BankId::new(0xFB),
        vram: 0x8000_0200,
        words: &eret_words,
    });
    let eret_return_words = [
        0x0800_2000, // j     0x80008000
        0x2407_004D, // addiu $a3,$zero,77 (delay)
    ];
    let emitted_eret_return = emit_bank_runner(&BankInput {
        name: "run_eret_return",
        bank: BankId::new(0xFC),
        vram: 0x8000_8000,
        words: &eret_return_words,
    });

    for pc in (BASE..BASE + WORDS.len() as u32 * 4).step_by(4) {
        assert!(
            emitted.contains(&format!("{pc:#010X} => {{")),
            "missing arbitrary-PC arm at {pc:#010X}\n{emitted}"
        );
    }
    assert!(emitted_sparse_bank.contains("0x80005000 => {"));
    assert!(emitted_sparse_bank.contains("0x80005020 => {"));
    assert!(
        !emitted_sparse_bank.contains("0x80005010 => {"),
        "a data hole must never receive an instruction arm:\n{emitted_sparse_bank}"
    );

    let source = format!(
        r#"
use fn64_cpu_runtime::{{
    BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CodeSpan, CpuFault,
    CpuException, CpuFaultKind, ExecutionKey, GeneratedBankRunner, GuestPc,
    InstructionBudget, ProgramError, Rdram, RecompContext,
}};

{emitted}
{emitted_leaf_bank}
{emitted_leaf_function}
{emitted_host_call_bank}
{emitted_dynamic_call_bank}
{emitted_sparse_bank}
{emitted_exception_bank}
{emitted_overflow_bank}
{emitted_address_bank}
{emitted_cop1_bank}
{emitted_fp_trap_bank}
{emitted_fp_delay_bank}
{emitted_exception_handler}
{emitted_fetch_handler}
{emitted_eret_bank}
{emitted_eret_return}

fn main() {{
    let mut storage = vec![0u8; 64];
    let mut mem = Rdram::new(&mut storage);

    let mut overflow_ctx = RecompContext::new();
    overflow_ctx.set_r32(2, i32::MAX);
    let overflow = run_overflow_bank(
        ExecutionKey::new(BankId::new(0xE9), GuestPc::new(0x8000_7000)),
        InstructionBudget::new(64).unwrap(),
        &mut overflow_ctx,
        &mut mem,
    );
    assert_eq!(overflow.instructions, 1);
    assert_eq!(overflow_ctx.r_s32(2), i32::MAX);
    assert!(matches!(
        overflow.exit,
        BlockExit::Fault(CpuFault {{
            kind: CpuFaultKind::Exception {{
                exception: CpuException::IntegerOverflow,
                epc,
                branch_delay: false,
                ..
            }},
            ..
        }}) if epc == GuestPc::new(0x8000_7000)
    ));

    let mut address_ctx = RecompContext::new();
    address_ctx.set_r(2, 0x1122_3344);
    address_ctx.set_r(4, 0xFFFF_FFFF_8000_0000);
    let address_load = run_address_bank(
        ExecutionKey::new(BankId::new(0xEA), GuestPc::new(0x8000_B000)),
        InstructionBudget::new(64).unwrap(),
        &mut address_ctx,
        &mut mem,
    );
    assert_eq!(address_load.instructions, 1);
    assert_eq!(address_ctx.r(2), 0x1122_3344);
    assert!(matches!(
        address_load.exit,
        BlockExit::Fault(CpuFault {{
            at: ExecutionKey {{ pc, .. }},
            kind: CpuFaultKind::Exception {{
                exception: CpuException::AddressErrorLoad,
                epc,
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(0xffff_ffff_8000_0001),
                coprocessor: None,
            }},
        }}) if pc == GuestPc::new(0x8000_B000) && epc == GuestPc::new(0x8000_B000)
    ));

    let address_store = run_address_bank(
        ExecutionKey::new(BankId::new(0xEA), GuestPc::new(0x8000_B004)),
        InstructionBudget::new(64).unwrap(),
        &mut address_ctx,
        &mut mem,
    );
    assert_eq!(address_store.instructions, 2);
    assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000), 0);
    assert!(matches!(
        address_store.exit,
        BlockExit::Fault(CpuFault {{
            at: ExecutionKey {{ pc, .. }},
            kind: CpuFaultKind::Exception {{
                exception: CpuException::AddressErrorStore,
                epc,
                branch_delay: true,
                instruction_code: 0,
                bad_vaddr: Some(0xffff_ffff_8000_0002),
                coprocessor: None,
            }},
        }}) if pc == GuestPc::new(0x8000_B008) && epc == GuestPc::new(0x8000_B004)
    ));

    let mut cop1_ctx = RecompContext::new();
    cop1_ctx.set_r(2, 0x1122_3344);
    cop1_ctx.set_f_bits(0, 0x3F80_0000);
    let cop1_straight = run_cop1_bank(
        ExecutionKey::new(BankId::new(0xEB), GuestPc::new(0x8000_D000)),
        InstructionBudget::new(64).unwrap(),
        &mut cop1_ctx,
        &mut mem,
    );
    assert_eq!(cop1_straight.instructions, 1);
    assert_eq!(cop1_ctx.r(2), 0x1122_3344);
    assert!(matches!(
        cop1_straight.exit,
        BlockExit::Fault(CpuFault {{
            at: ExecutionKey {{ pc, .. }},
            kind: CpuFaultKind::Exception {{
                exception: CpuException::CoprocessorUnusable,
                epc,
                branch_delay: false,
                bad_vaddr: None,
                coprocessor: Some(1),
                ..
            }},
        }}) if pc == GuestPc::new(0x8000_D000) && epc == GuestPc::new(0x8000_D000)
    ));

    let cop1_delay = run_cop1_bank(
        ExecutionKey::new(BankId::new(0xEB), GuestPc::new(0x8000_D004)),
        InstructionBudget::new(64).unwrap(),
        &mut cop1_ctx,
        &mut mem,
    );
    assert_eq!(cop1_delay.instructions, 2);
    assert_eq!(cop1_ctx.r(2), 0x1122_3344);
    assert!(matches!(
        cop1_delay.exit,
        BlockExit::Fault(CpuFault {{
            at: ExecutionKey {{ pc, .. }},
            kind: CpuFaultKind::Exception {{
                exception: CpuException::CoprocessorUnusable,
                epc,
                branch_delay: true,
                coprocessor: Some(1),
                ..
            }},
        }}) if pc == GuestPc::new(0x8000_D008) && epc == GuestPc::new(0x8000_D004)
    ));

    let cop1_branch = run_cop1_bank(
        ExecutionKey::new(BankId::new(0xEB), GuestPc::new(0x8000_D010)),
        InstructionBudget::new(64).unwrap(),
        &mut cop1_ctx,
        &mut mem,
    );
    assert_eq!(cop1_branch.instructions, 1);
    assert_eq!(cop1_ctx.r(4), 0);
    assert!(matches!(
        cop1_branch.exit,
        BlockExit::Fault(CpuFault {{
            at: ExecutionKey {{ pc, .. }},
            kind: CpuFaultKind::Exception {{
                exception: CpuException::CoprocessorUnusable,
                epc,
                branch_delay: false,
                coprocessor: Some(1),
                ..
            }},
        }}) if pc == GuestPc::new(0x8000_D010) && epc == GuestPc::new(0x8000_D010)
    ));

    cop1_ctx.cop0_status = 1 << 29;
    let cop1_enabled = run_cop1_bank(
        ExecutionKey::new(BankId::new(0xEB), GuestPc::new(0x8000_D000)),
        InstructionBudget::new(2).unwrap(),
        &mut cop1_ctx,
        &mut mem,
    );
    assert_eq!(cop1_enabled.instructions, 1);
    assert_eq!(cop1_ctx.r_u32(2), 0x3F80_0000);
    assert_eq!(
        cop1_enabled.exit,
        BlockExit::Checkpoint(ExecutionKey::new(
            BankId::new(0xEB),
            GuestPc::new(0x8000_D004),
        )),
    );

    cop1_ctx.fpu_cond = true;
    let cop1_branch_enabled = run_cop1_bank(
        ExecutionKey::new(BankId::new(0xEB), GuestPc::new(0x8000_D010)),
        InstructionBudget::new(2).unwrap(),
        &mut cop1_ctx,
        &mut mem,
    );
    assert_eq!(cop1_branch_enabled.instructions, 2);
    assert_eq!(cop1_ctx.r(4), 7);
    assert_eq!(
        cop1_branch_enabled.exit,
        BlockExit::Transfer(ExecutionKey::new(
            BankId::new(0xEB),
            GuestPc::new(0x8000_D018),
        )),
    );

    cop1_ctx.cop0_status = 0;
    cop1_ctx.set_r(4, 0xFFFF_FFFF_8000_0000);
    let cop1_priority = run_cop1_bank(
        ExecutionKey::new(BankId::new(0xEB), GuestPc::new(0x8000_D01C)),
        InstructionBudget::new(64).unwrap(),
        &mut cop1_ctx,
        &mut mem,
    );
    assert!(matches!(
        cop1_priority.exit,
        BlockExit::Fault(CpuFault {{
            kind: CpuFaultKind::Exception {{
                exception: CpuException::CoprocessorUnusable,
                coprocessor: Some(1),
                ..
            }},
            ..
        }})
    ));
    cop1_ctx.cop0_status = 1 << 29;
    let cop1_address = run_cop1_bank(
        ExecutionKey::new(BankId::new(0xEB), GuestPc::new(0x8000_D01C)),
        InstructionBudget::new(64).unwrap(),
        &mut cop1_ctx,
        &mut mem,
    );
    assert!(matches!(
        cop1_address.exit,
        BlockExit::Fault(CpuFault {{
            kind: CpuFaultKind::Exception {{
                exception: CpuException::AddressErrorLoad,
                bad_vaddr: Some(0xffff_ffff_8000_0001),
                coprocessor: None,
                ..
            }},
            ..
        }})
    ));

    // --- Enabled FP exception (ExcCode 15) in the block lane. ---
    // COP1 usable (CU1), Enable.V set, $f2 = -1.0, $f0 seeded with a sentinel.
    let mut fp_ctx = RecompContext::new();
    fp_ctx.cop0_status = 1 << 29;
    fp_ctx.write_fcr(31, 1 << 11); // Enable.V (Invalid)
    fp_ctx.set_f_s(2, -1.0);
    fp_ctx.set_f_bits(0, 0xDEAD_BEEF);
    let fp_trap = run_fp_trap_bank(
        ExecutionKey::new(BankId::new(0xEC), GuestPc::new(0x8000_E000)),
        InstructionBudget::new(64).unwrap(),
        &mut fp_ctx,
        &mut mem,
    );
    assert_eq!(fp_trap.instructions, 1);
    assert_eq!(
        fp_ctx.f_bits(0),
        0xDEAD_BEEF,
        "destination register must NOT be written on an enabled FP trap"
    );
    // Cause.V is recorded (bit 16) but the sticky Flag.V (bit 6) is not.
    let fp_fcsr = fp_ctx.read_fcr(31);
    assert_ne!(fp_fcsr & (1 << 16), 0, "FCSR Cause.V set on the trapped op");
    assert_eq!(fp_fcsr & (1 << 6), 0, "FCSR Flag.V NOT set on the trapped op");
    assert!(matches!(
        fp_trap.exit,
        BlockExit::Fault(CpuFault {{
            at: ExecutionKey {{ pc, .. }},
            kind: CpuFaultKind::Exception {{
                exception: CpuException::FloatingPoint,
                epc,
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: None,
                coprocessor: None,
            }},
        }}) if pc == GuestPc::new(0x8000_E000) && epc == GuestPc::new(0x8000_E000)
    ));

    // The same trap in a branch delay slot: Cause.BD set, EPC = the branch.
    let mut fp_delay_ctx = RecompContext::new();
    fp_delay_ctx.cop0_status = 1 << 29;
    fp_delay_ctx.write_fcr(31, 1 << 11);
    fp_delay_ctx.set_f_s(2, -1.0);
    fp_delay_ctx.set_f_bits(0, 0xDEAD_BEEF);
    let fp_delay = run_fp_delay_bank(
        ExecutionKey::new(BankId::new(0xED), GuestPc::new(0x8000_F000)),
        InstructionBudget::new(64).unwrap(),
        &mut fp_delay_ctx,
        &mut mem,
    );
    assert_eq!(fp_delay.instructions, 2);
    assert_eq!(
        fp_delay_ctx.f_bits(0),
        0xDEAD_BEEF,
        "delay-slot FP trap must not write the destination"
    );
    assert!(matches!(
        fp_delay.exit,
        BlockExit::Fault(CpuFault {{
            at: ExecutionKey {{ pc, .. }},
            kind: CpuFaultKind::Exception {{
                exception: CpuException::FloatingPoint,
                epc,
                branch_delay: true,
                coprocessor: None,
                ..
            }},
        }}) if pc == GuestPc::new(0x8000_F004) && epc == GuestPc::new(0x8000_F000)
    ));

    // The disabled path is unchanged: no trap, result committed, sticky Flag set.
    let mut fp_ok_ctx = RecompContext::new();
    fp_ok_ctx.cop0_status = 1 << 29;
    fp_ok_ctx.set_f_s(2, -1.0);
    fp_ok_ctx.set_f_bits(0, 0xDEAD_BEEF);
    let fp_ok = run_fp_trap_bank(
        ExecutionKey::new(BankId::new(0xEC), GuestPc::new(0x8000_E000)),
        InstructionBudget::new(2).unwrap(),
        &mut fp_ok_ctx,
        &mut mem,
    );
    assert_eq!(
        fp_ok_ctx.f_bits(0),
        0x7FBF_FFFF,
        "disabled FP exception commits the canonical-NaN result"
    );
    let fp_ok_fcsr = fp_ok_ctx.read_fcr(31);
    assert_ne!(fp_ok_fcsr & (1 << 16), 0, "Cause.V set (disabled)");
    assert_ne!(fp_ok_fcsr & (1 << 6), 0, "Flag.V set (disabled, committed)");
    // The disabled op does NOT fault: the block continues past the sqrt.
    assert!(
        !matches!(
            fp_ok.exit,
            BlockExit::Fault(CpuFault {{
                kind: CpuFaultKind::Exception {{
                    exception: CpuException::FloatingPoint,
                    ..
                }},
                ..
            }})
        ),
        "a disabled FP exception must not fault"
    );

    let syscall = run_exception_bank(
        ExecutionKey::new(BankId::new(0xD8), GuestPc::new(0x8000_6000)),
        InstructionBudget::new(64).unwrap(),
        &mut RecompContext::new(),
        &mut mem,
    );
    assert_eq!(syscall.instructions, 1);
    assert!(matches!(
        syscall.exit,
        BlockExit::Fault(CpuFault {{
            at: ExecutionKey {{ pc, .. }},
            kind: CpuFaultKind::Exception {{
                exception: CpuException::Syscall,
                epc,
                branch_delay: false,
                instruction_code: 0x123,
                bad_vaddr: None,
                coprocessor: None,
            }},
        }}) if pc == GuestPc::new(0x8000_6000) && epc == GuestPc::new(0x8000_6000)
    ));

    let mut trap_ctx = RecompContext::new();
    trap_ctx.set_r(4, 9);
    trap_ctx.set_r(5, 9);
    let conditional_trap = run_exception_bank(
        ExecutionKey::new(BankId::new(0xD8), GuestPc::new(0x8000_6010)),
        InstructionBudget::new(64).unwrap(),
        &mut trap_ctx,
        &mut mem,
    );
    assert_eq!(conditional_trap.instructions, 1);
    assert!(matches!(
        conditional_trap.exit,
        BlockExit::Fault(CpuFault {{
            kind: CpuFaultKind::Exception {{
                exception: CpuException::Trap,
                branch_delay: false,
                instruction_code: 0x55,
                ..
            }},
            ..
        }})
    ));

    trap_ctx.set_r(5, 10);
    let no_trap = run_exception_bank(
        ExecutionKey::new(BankId::new(0xD8), GuestPc::new(0x8000_6010)),
        InstructionBudget::new(64).unwrap(),
        &mut trap_ctx,
        &mut mem,
    );
    assert_eq!(no_trap.instructions, 2);
    assert_eq!(
        no_trap.exit,
        BlockExit::ResolveTransfer {{
            source_bank: BankId::new(0xD8),
            target_pc: GuestPc::new(0x8000_6018),
        }}
    );

    let delay_break = run_exception_bank(
        ExecutionKey::new(BankId::new(0xD8), GuestPc::new(0x8000_6004)),
        InstructionBudget::new(64).unwrap(),
        &mut RecompContext::new(),
        &mut mem,
    );
    assert_eq!(delay_break.instructions, 2);
    assert!(matches!(
        delay_break.exit,
        BlockExit::Fault(CpuFault {{
            at: ExecutionKey {{ pc, .. }},
            kind: CpuFaultKind::Exception {{
                exception: CpuException::Breakpoint,
                epc,
                branch_delay: true,
                instruction_code: 7,
                bad_vaddr: None,
                coprocessor: None,
            }},
        }}) if pc == GuestPc::new(0x8000_6008) && epc == GuestPc::new(0x8000_6004)
    ));

    // The program dispatcher commits CP0 exception state, resolves the
    // general vector through the active mapping, executes a real EPC
    // read/adjust/write handler, and follows ERET back to the faulting bank.
    let exception_id = BankId::new(0xD8);
    let handler_id = BankId::new(0xFA);
    let mut exception_program = BlockProgram::new();
    register_run_exception_bank(
        &mut exception_program,
        CodeBank::new(
            exception_id,
            GuestPc::new(0x8000_6000),
            vec!{exception_words:?},
        ).unwrap(),
    ).unwrap();
    register_run_exception_handler(
        &mut exception_program,
        CodeBank::new(
            handler_id,
            GuestPc::new(0x8000_0180),
            vec!{exception_handler_words:?},
        ).unwrap(),
    ).unwrap();
    let mut dispatched_ctx = RecompContext::new();
    let mut resolve_exception = |source: BankId, target: GuestPc| {{
        match (source, target) {{
            (source, target) if source == exception_id && target == GuestPc::new(0x8000_0180) =>
                Ok(ExecutionKey::new(handler_id, target)),
            (source, target) if source == handler_id && target == GuestPc::new(0x8000_6004) =>
                Ok(ExecutionKey::new(exception_id, target)),
            _ => panic!("unexpected exception transfer from {{source:?}} to {{target:?}}"),
        }}
    }};
    let dispatched = exception_program.dispatch(
        ExecutionKey::new(exception_id, GuestPc::new(0x8000_6000)),
        InstructionBudget::new(5).unwrap(),
        &mut dispatched_ctx,
        &mut mem,
        &mut resolve_exception,
    ).unwrap();
    assert_eq!(dispatched_ctx.r_u32(8), 0x8000_6004);
    assert_eq!(dispatched_ctx.cop0_epc, 0x8000_6004);
    assert_eq!((dispatched_ctx.cop0_cause >> 2) & 0x1F, 8);
    assert_eq!(dispatched_ctx.cop0_status & (1 << 1), 0);
    assert_eq!(dispatched.instructions, 5);
    assert_eq!(dispatched.blocks, 2);
    assert_eq!(
        dispatched.exit,
        BlockExit::Checkpoint(ExecutionKey::new(
            exception_id,
            GuestPc::new(0x8000_6004),
        )),
    );

    // Address errors use the same installed guest exception vector. The
    // handler observes precise CP0 state and ERET resumes after the fault.
    let address_id = BankId::new(0xEA);
    let mut address_program = BlockProgram::new();
    register_run_address_bank(
        &mut address_program,
        CodeBank::new(
            address_id,
            GuestPc::new(0x8000_B000),
            vec!{address_words:?},
        ).unwrap(),
    ).unwrap();
    register_run_exception_handler(
        &mut address_program,
        CodeBank::new(
            handler_id,
            GuestPc::new(0x8000_0180),
            vec!{exception_handler_words:?},
        ).unwrap(),
    ).unwrap();
    let mut address_dispatch_ctx = RecompContext::new();
    address_dispatch_ctx.set_r(4, 0xFFFF_FFFF_8000_0000);
    let mut resolve_address = |source: BankId, target: GuestPc| {{
        match (source, target) {{
            (source, target) if source == address_id && target == GuestPc::new(0x8000_0180) =>
                Ok(ExecutionKey::new(handler_id, target)),
            (source, target) if source == handler_id && target == GuestPc::new(0x8000_B004) =>
                Ok(ExecutionKey::new(address_id, target)),
            _ => panic!("unexpected address-exception transfer from {{source:?}} to {{target:?}}"),
        }}
    }};
    let address_dispatched = address_program.dispatch(
        ExecutionKey::new(address_id, GuestPc::new(0x8000_B000)),
        InstructionBudget::new(5).unwrap(),
        &mut address_dispatch_ctx,
        &mut mem,
        &mut resolve_address,
    ).unwrap();
    assert_eq!(address_dispatch_ctx.cop0_badvaddr, 0xffff_ffff_8000_0001);
    assert_eq!(address_dispatch_ctx.cop0_epc, 0x8000_B004);
    assert_eq!((address_dispatch_ctx.cop0_cause >> 2) & 0x1F, 4);
    assert_eq!(address_dispatch_ctx.cop0_cause & (1 << 31), 0);
    assert_eq!(address_dispatch_ctx.cop0_status & (1 << 1), 0);
    assert_eq!(address_dispatched.instructions, 5);
    assert_eq!(address_dispatched.blocks, 2);
    assert_eq!(
        address_dispatched.exit,
        BlockExit::Checkpoint(ExecutionKey::new(
            address_id,
            GuestPc::new(0x8000_B004),
        )),
    );

    let cop1_id = BankId::new(0xEB);
    let mut cop1_program = BlockProgram::new();
    register_run_cop1_bank(
        &mut cop1_program,
        CodeBank::new(
            cop1_id,
            GuestPc::new(0x8000_D000),
            vec!{cop1_words:?},
        ).unwrap(),
    ).unwrap();
    register_run_exception_handler(
        &mut cop1_program,
        CodeBank::new(
            handler_id,
            GuestPc::new(0x8000_0180),
            vec!{exception_handler_words:?},
        ).unwrap(),
    ).unwrap();
    let mut cop1_dispatch_ctx = RecompContext::new();
    let mut resolve_cop1 = |source: BankId, target: GuestPc| {{
        match (source, target) {{
            (source, target) if source == cop1_id && target == GuestPc::new(0x8000_0180) =>
                Ok(ExecutionKey::new(handler_id, target)),
            (source, target) if source == handler_id && target == GuestPc::new(0x8000_D004) =>
                Ok(ExecutionKey::new(cop1_id, target)),
            _ => panic!("unexpected COP1-exception transfer from {{source:?}} to {{target:?}}"),
        }}
    }};
    let cop1_dispatched = cop1_program.dispatch(
        ExecutionKey::new(cop1_id, GuestPc::new(0x8000_D000)),
        InstructionBudget::new(5).unwrap(),
        &mut cop1_dispatch_ctx,
        &mut mem,
        &mut resolve_cop1,
    ).unwrap();
    assert_eq!((cop1_dispatch_ctx.cop0_cause >> 2) & 0x1F, 11);
    assert_eq!((cop1_dispatch_ctx.cop0_cause >> 28) & 0b11, 1);
    assert_eq!(cop1_dispatch_ctx.cop0_epc, 0x8000_D004);
    assert_eq!(cop1_dispatch_ctx.cop0_status & (1 << 1), 0);
    assert_eq!(cop1_dispatched.instructions, 5);
    assert_eq!(cop1_dispatched.blocks, 2);
    assert_eq!(
        cop1_dispatched.exit,
        BlockExit::Checkpoint(ExecutionKey::new(
            cop1_id,
            GuestPc::new(0x8000_D004),
        )),
    );

    // An enabled FP exception drives the SAME installed guest vector: the
    // dispatcher commits ExcCode-15 CP0 state, enters the handler, and ERET
    // resumes at the handler-selected EPC. The destination register the trapping
    // sqrt would have written stays untouched across the whole flow.
    let fp_id = BankId::new(0xEC);
    let mut fp_program = BlockProgram::new();
    register_run_fp_trap_bank(
        &mut fp_program,
        CodeBank::new(
            fp_id,
            GuestPc::new(0x8000_E000),
            vec!{fp_trap_words:?},
        ).unwrap(),
    ).unwrap();
    register_run_exception_handler(
        &mut fp_program,
        CodeBank::new(
            handler_id,
            GuestPc::new(0x8000_0180),
            vec!{exception_handler_words:?},
        ).unwrap(),
    ).unwrap();
    let mut fp_dispatch_ctx = RecompContext::new();
    fp_dispatch_ctx.cop0_status = 1 << 29; // CU1 usable
    fp_dispatch_ctx.write_fcr(31, 1 << 11); // Enable.V
    fp_dispatch_ctx.set_f_s(2, -1.0);
    fp_dispatch_ctx.set_f_bits(0, 0xDEAD_BEEF);
    let mut resolve_fp = |source: BankId, target: GuestPc| {{
        match (source, target) {{
            (source, target) if source == fp_id && target == GuestPc::new(0x8000_0180) =>
                Ok(ExecutionKey::new(handler_id, target)),
            (source, target) if source == handler_id && target == GuestPc::new(0x8000_E004) =>
                Ok(ExecutionKey::new(fp_id, target)),
            _ => panic!("unexpected FP-exception transfer from {{source:?}} to {{target:?}}"),
        }}
    }};
    let fp_dispatched = fp_program.dispatch(
        ExecutionKey::new(fp_id, GuestPc::new(0x8000_E000)),
        InstructionBudget::new(5).unwrap(),
        &mut fp_dispatch_ctx,
        &mut mem,
        &mut resolve_fp,
    ).unwrap();
    assert_eq!((fp_dispatch_ctx.cop0_cause >> 2) & 0x1F, 15, "ExcCode 15 (FPE)");
    assert_eq!((fp_dispatch_ctx.cop0_cause >> 28) & 0b11, 0, "Cause.CE not set for FPE");
    assert_eq!(fp_dispatch_ctx.cop0_epc, 0x8000_E004, "EPC advanced by the handler");
    assert_eq!(fp_dispatch_ctx.cop0_cause & (1 << 31), 0, "Cause.BD clear (straight-line)");
    assert_eq!(fp_dispatch_ctx.cop0_status & (1 << 1), 0, "EXL cleared by ERET");
    assert_eq!(fp_dispatch_ctx.f_bits(0), 0xDEAD_BEEF, "trapping sqrt never wrote $f0");
    assert_ne!(fp_dispatch_ctx.read_fcr(31) & (1 << 16), 0, "FCSR Cause.V recorded");
    assert_eq!(fp_dispatch_ctx.read_fcr(31) & (1 << 6), 0, "FCSR Flag.V not sticky on trap");
    assert_eq!(fp_dispatched.instructions, 5);
    assert_eq!(fp_dispatched.blocks, 2);
    assert_eq!(
        fp_dispatched.exit,
        BlockExit::Checkpoint(ExecutionKey::new(
            fp_id,
            GuestPc::new(0x8000_E004),
        )),
    );

    // ERET is a typed no-delay transfer in the block lane: it clears EXL and
    // LLbit before resolving EPC, then execution continues in that code bank.
    let eret_id = BankId::new(0xFB);
    let eret_return_id = BankId::new(0xFC);
    let mut eret_program = BlockProgram::new();
    register_run_eret_bank(
        &mut eret_program,
        CodeBank::new(
            eret_id,
            GuestPc::new(0x8000_0200),
            vec!{eret_words:?},
        ).unwrap(),
    ).unwrap();
    register_run_eret_return(
        &mut eret_program,
        CodeBank::new(
            eret_return_id,
            GuestPc::new(0x8000_8000),
            vec!{eret_return_words:?},
        ).unwrap(),
    ).unwrap();
    let mut eret_ctx = RecompContext::new();
    eret_ctx.cop0_status = 1 << 1;
    eret_ctx.cop0_epc = 0x8000_8000;
    eret_ctx.set_ll_reservation(0x8000_0040, 4);
    let mut resolve_eret = |source: BankId, target: GuestPc| {{
        assert_eq!(source, eret_id);
        assert_eq!(target, GuestPc::new(0x8000_8000));
        Ok(ExecutionKey::new(eret_return_id, target))
    }};
    let eret_run = eret_program.dispatch(
        ExecutionKey::new(eret_id, GuestPc::new(0x8000_0200)),
        InstructionBudget::new(3).unwrap(),
        &mut eret_ctx,
        &mut mem,
        &mut resolve_eret,
    ).unwrap();
    assert_eq!(eret_ctx.cop0_status & (1 << 1), 0);
    assert!(!eret_ctx.take_ll_reservation(0x8000_0040, 4));
    assert_eq!(eret_ctx.r_u32(7), 77);
    assert_eq!(eret_run.instructions, 3);
    assert_eq!(eret_run.blocks, 2);
    assert_eq!(
        eret_run.exit,
        BlockExit::Yield(ExecutionKey::new(
            eret_return_id,
            GuestPc::new(0x8000_8000),
        )),
    );

    // The historical function start is irrelevant: enter at the second word.
    let mut ctx = RecompContext::new();
    ctx.set_r(2, 10);
    let exit = run_test_bank(
        ExecutionKey::new(BankId::new(0xA5), GuestPc::new(0x8000_1004)),
        InstructionBudget::new(64).unwrap(),
        &mut ctx,
        &mut mem,
    );
    assert_eq!(ctx.r(2), 12);
    assert_eq!(ctx.r(4), 7);
    assert_eq!(
        exit.exit,
        BlockExit::Transfer(ExecutionKey::new(
            BankId::new(0xA5),
            GuestPc::new(0x8000_1010),
        )),
    );
    assert_eq!(exit.instructions, 3);

    // A deterministic budget stops before a branch+delay pair rather than
    // splitting that architectural unit.
    let mut checkpoint_ctx = RecompContext::new();
    let checkpoint = run_test_bank(
        ExecutionKey::new(BankId::new(0xA5), GuestPc::new(0x8000_1000)),
        InstructionBudget::new(2).unwrap(),
        &mut checkpoint_ctx,
        &mut mem,
    );
    assert_eq!(checkpoint_ctx.r(2), 3);
    assert_eq!(checkpoint_ctx.r(4), 0);
    assert_eq!(checkpoint.instructions, 2);
    assert_eq!(
        checkpoint.exit,
        BlockExit::Checkpoint(ExecutionKey::new(
            BankId::new(0xA5),
            GuestPc::new(0x8000_1008),
        )),
    );

    // JR snapshots its target before a delay slot that overwrites the source.
    ctx.set_r(8, 0x8000_2000);
    let exit = run_test_bank(
        ExecutionKey::new(BankId::new(0xA5), GuestPc::new(0x8000_1010)),
        InstructionBudget::new(64).unwrap(),
        &mut ctx,
        &mut mem,
    );
    assert_eq!(ctx.r_u32(8), 0x1234);
    assert_eq!(
        exit.exit,
        BlockExit::ResolveTransfer {{
            source_bank: BankId::new(0xA5),
            target_pc: GuestPc::new(0x8000_2000),
        }},
    );
    assert_eq!(exit.instructions, 2);

    let wrong_bank = run_test_bank(
        ExecutionKey::new(BankId::new(0xA6), GuestPc::new(0x8000_1000)),
        InstructionBudget::new(64).unwrap(),
        &mut ctx,
        &mut mem,
    );
    assert!(matches!(
        wrong_bank.exit,
        BlockExit::Fault(CpuFault {{ kind: CpuFaultKind::UnknownBank, .. }})
    ));
    assert_eq!(wrong_bank.instructions, 0);

    let unaligned = run_test_bank(
        ExecutionKey::new(BankId::new(0xA5), GuestPc::new(0x8000_1002)),
        InstructionBudget::new(64).unwrap(),
        &mut ctx,
        &mut mem,
    );
    assert!(matches!(
        unaligned.exit,
        BlockExit::Fault(CpuFault {{
            at: ExecutionKey {{ pc, .. }},
            kind: CpuFaultKind::Exception {{
                exception: CpuException::AddressErrorLoad,
                epc,
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(0x8000_1002),
                coprocessor: None,
            }},
        }}) if pc == GuestPc::new(0x8000_1002) && epc == GuestPc::new(0x8000_1002)
    ));
    assert_eq!(unaligned.instructions, 1);

    // A computed unaligned target faults on the following instruction-fetch
    // attempt, after the branch and its delay slot have retired. If the pair
    // exactly exhausts the budget, that fetch is checkpointed first.
    ctx.set_r(8, 0x8000_2002);
    let unaligned_target = run_test_bank(
        ExecutionKey::new(BankId::new(0xA5), GuestPc::new(0x8000_1010)),
        InstructionBudget::new(64).unwrap(),
        &mut ctx,
        &mut mem,
    );
    assert_eq!(ctx.r_u32(8), 0x1234);
    assert_eq!(unaligned_target.instructions, 3);
    assert!(matches!(
        unaligned_target.exit,
        BlockExit::Fault(CpuFault {{
            at: ExecutionKey {{ pc, .. }},
            kind: CpuFaultKind::Exception {{
                exception: CpuException::AddressErrorLoad,
                epc,
                branch_delay: false,
                bad_vaddr: Some(0x8000_2002),
                ..
            }},
        }}) if pc == GuestPc::new(0x8000_2002) && epc == GuestPc::new(0x8000_2002)
    ));

    ctx.set_r(8, 0x8000_2002);
    let checkpointed_fetch = run_test_bank(
        ExecutionKey::new(BankId::new(0xA5), GuestPc::new(0x8000_1010)),
        InstructionBudget::new(2).unwrap(),
        &mut ctx,
        &mut mem,
    );
    assert_eq!(checkpointed_fetch.instructions, 2);
    assert_eq!(
        checkpointed_fetch.exit,
        BlockExit::Checkpoint(ExecutionKey::new(
            BankId::new(0xA5),
            GuestPc::new(0x8000_2002),
        )),
    );

    // The dispatcher applies fetch AdEL to CP0, enters a registered guest
    // vector, and resumes at the aligned EPC selected by that handler.
    let fetch_code_id = BankId::new(0xA5);
    let fetch_handler_id = BankId::new(0xF9);
    let mut fetch_program = BlockProgram::new();
    register_run_test_bank(
        &mut fetch_program,
        CodeBank::new(fetch_code_id, GuestPc::new(0x8000_1000), vec!{WORDS:?}).unwrap(),
    ).unwrap();
    register_run_fetch_handler(
        &mut fetch_program,
        CodeBank::new(
            fetch_handler_id,
            GuestPc::new(0x8000_0180),
            vec!{fetch_handler_words:?},
        ).unwrap(),
    ).unwrap();
    let mut fetch_dispatch_ctx = RecompContext::new();
    let mut resolve_fetch = |source: BankId, target: GuestPc| {{
        match (source, target) {{
            (source, target) if source == fetch_code_id && target == GuestPc::new(0x8000_0180) =>
                Ok(ExecutionKey::new(fetch_handler_id, target)),
            (source, target) if source == fetch_handler_id && target == GuestPc::new(0x8000_1004) =>
                Ok(ExecutionKey::new(fetch_code_id, target)),
            _ => panic!("unexpected fetch-exception transfer from {{source:?}} to {{target:?}}"),
        }}
    }};
    let fetch_dispatched = fetch_program.dispatch(
        ExecutionKey::new(fetch_code_id, GuestPc::new(0x8000_1002)),
        InstructionBudget::new(5).unwrap(),
        &mut fetch_dispatch_ctx,
        &mut mem,
        &mut resolve_fetch,
    ).unwrap();
    assert_eq!(fetch_dispatch_ctx.cop0_badvaddr, 0x8000_1002);
    assert_eq!(fetch_dispatch_ctx.cop0_epc, 0x8000_1004);
    assert_eq!((fetch_dispatch_ctx.cop0_cause >> 2) & 0x1F, 4);
    assert_eq!(fetch_dispatch_ctx.cop0_cause & (1 << 31), 0);
    assert_eq!(fetch_dispatch_ctx.cop0_status & (1 << 1), 0);
    assert_eq!(fetch_dispatched.instructions, 5);
    assert_eq!(fetch_dispatched.blocks, 2);
    assert_eq!(
        fetch_dispatched.exit,
        BlockExit::Checkpoint(ExecutionKey::new(
            fetch_code_id,
            GuestPc::new(0x8000_1004),
        )),
    );

    // At an ordinary function entry, both codegen lanes execute the same
    // instruction semantics. The block lane exposes JR $ra as a transfer;
    // the historical function lane returns to its native caller.
    let mut function_storage = vec![0u8; 64];
    let mut function_mem = Rdram::new(&mut function_storage);
    let mut function_ctx = RecompContext::new();
    function_ctx.set_r(31, 0x8000_4000);
    run_leaf_function(&mut function_ctx, &mut function_mem);

    let mut block_storage = vec![0u8; 64];
    let mut block_mem = Rdram::new(&mut block_storage);
    let mut block_ctx = RecompContext::new();
    block_ctx.set_r(31, 0x8000_4000);
    let leaf_exit = run_leaf_bank(
        ExecutionKey::new(BankId::new(0xB6), GuestPc::new(0x8000_3000)),
        InstructionBudget::new(64).unwrap(),
        &mut block_ctx,
        &mut block_mem,
    );
    assert_eq!(block_ctx.gprs(), function_ctx.gprs());
    assert_eq!(
        leaf_exit.exit,
        BlockExit::ResolveTransfer {{
            source_bank: BankId::new(0xB6),
            target_pc: GuestPc::new(0x8000_4000),
        }},
    );
    assert_eq!(leaf_exit.instructions, 3);

    // Only the explicit OSThread return sentinel terminates a live block
    // coroutine. An arbitrary unmapped return address above remained a
    // ResolveTransfer boundary.
    let mut return_storage = vec![0u8; 64];
    let mut return_mem = Rdram::new(&mut return_storage);
    let mut return_ctx = RecompContext::new();
    return_ctx.set_r32(31, -4);
    return_ctx.set_thread_return_pc(Some(0xFFFF_FFFC));
    let returned = run_leaf_bank(
        ExecutionKey::new(BankId::new(0xB6), GuestPc::new(0x8000_3000)),
        InstructionBudget::new(64).unwrap(),
        &mut return_ctx,
        &mut return_mem,
    );
    assert_eq!(returned.exit, BlockExit::ThreadReturn);
    assert_eq!(returned.instructions, 3);

    let mut host_storage = vec![0u8; 64];
    let mut host_mem = Rdram::new(&mut host_storage);
    let mut host_ctx = RecompContext::new();
    let host_call = run_host_call_bank(
        ExecutionKey::new(BankId::new(0xBD), GuestPc::new(0x8000_9000)),
        InstructionBudget::new(64).unwrap(),
        &mut host_ctx,
        &mut host_mem,
    );
    assert_eq!(host_ctx.r_u32(4), 7, "host-call delay slot must execute");
    assert_eq!(host_ctx.r_u32(31), 0x8000_9008);
    assert_eq!(
        host_call.exit,
        BlockExit::HostCall {{
            vram: GuestPc::new(0x8000_2000),
            resume: ExecutionKey::new(BankId::new(0xBD), GuestPc::new(0x8000_9008)),
        }},
    );
    assert_eq!(host_call.instructions, 2);

    let mut dynamic_storage = vec![0u8; 64];
    let mut dynamic_mem = Rdram::new(&mut dynamic_storage);
    let mut dynamic_ctx = RecompContext::new();
    dynamic_ctx.set_r32(25, 0x8000_2000u32 as i32);
    let dynamic_call = run_dynamic_call_bank(
        ExecutionKey::new(BankId::new(0xBE), GuestPc::new(0x8000_A000)),
        InstructionBudget::new(64).unwrap(),
        &mut dynamic_ctx,
        &mut dynamic_mem,
    );
    assert_eq!(dynamic_ctx.r_u32(4), 8);
    assert_eq!(dynamic_ctx.r_u32(31), 0x8000_A008);
    assert_eq!(
        dynamic_call.exit,
        BlockExit::ResolveCall {{
            source_bank: BankId::new(0xBE),
            target_pc: GuestPc::new(0x8000_2000),
            resume: ExecutionKey::new(BankId::new(0xBE), GuestPc::new(0x8000_A008)),
        }},
    );
    assert_eq!(dynamic_call.instructions, 2);

    // A static jump into another admitted span remains bank-qualified.
    let mut sparse_ctx = RecompContext::new();
    let sparse_jump = run_sparse_bank(
        ExecutionKey::new(BankId::new(0xC7), GuestPc::new(0x8000_5000)),
        InstructionBudget::new(64).unwrap(),
        &mut sparse_ctx,
        &mut mem,
    );
    assert_eq!(sparse_ctx.r_u32(4), 9);
    assert_eq!(
        sparse_jump.exit,
        BlockExit::Transfer(ExecutionKey::new(
            BankId::new(0xC7),
            GuestPc::new(0x8000_5020),
        )),
    );
    assert_eq!(sparse_jump.instructions, 2);

    // An aligned address between admitted spans is data/unclassified, not
    // executable merely because it lies inside the diagnostic bank bounds.
    let sparse_hole = run_sparse_bank(
        ExecutionKey::new(BankId::new(0xC7), GuestPc::new(0x8000_5010)),
        InstructionBudget::new(64).unwrap(),
        &mut sparse_ctx,
        &mut mem,
    );
    assert!(matches!(
        sparse_hole.exit,
        BlockExit::Fault(CpuFault {{
            kind: CpuFaultKind::UnmappedPc {{ .. }},
            ..
        }})
    ));
    assert_eq!(sparse_hole.instructions, 0);

    // Computed transfers into the same hole must go back through the active
    // mapping resolver, never acquire same-bank proof from bounding geometry.
    sparse_ctx.set_r(8, 0x8000_5010);
    let sparse_computed = run_sparse_bank(
        ExecutionKey::new(BankId::new(0xC7), GuestPc::new(0x8000_5020)),
        InstructionBudget::new(64).unwrap(),
        &mut sparse_ctx,
        &mut mem,
    );
    assert_eq!(sparse_ctx.r_u32(8), 0x1234);
    assert_eq!(
        sparse_computed.exit,
        BlockExit::ResolveTransfer {{
            source_bank: BankId::new(0xC7),
            target_pc: GuestPc::new(0x8000_5010),
        }},
    );
    assert_eq!(sparse_computed.instructions, 2);

    // The emitted registration helper binds this callable's embedded BankId
    // to the separately digest-verified sparse CodeBank. The program then
    // checks the catalog before invoking generated code.
    let sparse_id = BankId::new(0xC7);
    let sparse_code = CodeBank::from_spans(
        sparse_id,
        vec![
            CodeSpan::new(sparse_id, GuestPc::new(0x8000_5000), vec![0x0800_1408, 0x2404_0009]).unwrap(),
            CodeSpan::new(sparse_id, GuestPc::new(0x8000_5020), vec![0x0100_0008, 0x2408_1234]).unwrap(),
        ],
    ).unwrap();
    let mut program = BlockProgram::new();
    register_run_sparse_bank(&mut program, sparse_code).unwrap();
    let mut registered_ctx = RecompContext::new();
    let registered = program.run(
        ExecutionKey::new(sparse_id, GuestPc::new(0x8000_5000)),
        InstructionBudget::new(64).unwrap(),
        &mut registered_ctx,
        &mut mem,
    );
    assert_eq!(registered.instructions, 2);
    assert_eq!(registered_ctx.r_u32(4), 9);
}}
"#
    );

    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let source_path = out_dir.join("fn64_bank_runner_gate.rs");
    let binary_path = out_dir.join("fn64_bank_runner_gate");
    std::fs::write(&source_path, source).expect("write generated runner gate source");

    let deps = std::env::current_exe()
        .expect("current integration-test executable")
        .parent()
        .expect("target deps directory")
        .to_path_buf();
    let rlib = dev_interpreter_rlib(&deps);
    let compile = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("--edition=2021")
        .arg(&source_path)
        .arg("--extern")
        .arg(format!("fn64_cpu_runtime={}", rlib.display()))
        .arg("-L")
        .arg(format!("dependency={}", deps.display()))
        .arg("-o")
        .arg(&binary_path)
        .output()
        .expect("invoke rustc for generated bank runner");
    assert!(
        compile.status.success(),
        "generated bank runner did not compile:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&binary_path)
        .output()
        .expect("run generated bank runner gate");
    assert!(
        run.status.success(),
        "generated bank runner failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}
