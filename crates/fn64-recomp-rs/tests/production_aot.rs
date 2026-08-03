#![cfg(not(feature = "dev-interpreter"))]

use fn64_recomp_rs::{
    BankId, BlockExit, BlockProgram, CpuFaultKind, ExecutionKey, GuestPc, InstructionBudget,
    PhysicalCodeBank, Rdram, RecompContext,
};

#[test]
fn admitted_physical_code_without_aot_entry_fails_closed() {
    let bank = BankId::new(0xA07);
    let pc = GuestPc::new(0x8000_0040);
    let mut program = BlockProgram::new();
    program
        .register_physical_code(PhysicalCodeBank::new(bank, 0x40, vec![0x2402_0001]).unwrap())
        .unwrap();
    let mut ctx = RecompContext::new();
    let mut storage = [0u8; 8];
    let run = program.run(
        ExecutionKey::new(bank, pc),
        InstructionBudget::new(2).unwrap(),
        &mut ctx,
        &mut Rdram::new(&mut storage),
    );

    assert_eq!(run.instructions, 0);
    assert!(matches!(
        run.exit,
        BlockExit::Fault(fn64_recomp_rs::CpuFault {
            kind: CpuFaultKind::MissingAotEntry,
            ..
        })
    ));
}
