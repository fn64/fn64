//! Compile-and-run gate for bank-qualified arbitrary-PC emission.

use std::path::{Path, PathBuf};
use std::process::Command;

use fn64_recomp_rs::{
    classify_bank_words, emit_bank_runner, emit_sparse_bank_runner, BankBlockInput, BankId,
    BankInput, BankWordCatalog, BankWordKind, SparseBankInput,
};

const BASE: u32 = 0x8000_1000;
const WORDS: [u32; 6] = [
    0x2402_0001, // addiu $v0,$zero,1
    0x2442_0002, // addiu $v0,$v0,2
    0x1042_0001, // beq   $v0,$v0,0x80001010
    0x2404_0007, // addiu $a0,$zero,7 (delay)
    0x0100_0008, // jr    $t0
    0x2408_1234, // addiu $t0,$zero,0x1234 (delay; must not replace target)
];

#[test]
fn complete_bank_scan_keeps_unknown_and_control_words_explicit() {
    assert_eq!(
        classify_bank_words(&[0x2402_0001, 0x0100_0008, 0x7801_2345]),
        vec![
            BankWordKind::Straight,
            BankWordKind::ControlTransfer,
            BankWordKind::Unknown,
        ]
    );
}

#[test]
fn compact_catalog_resolves_only_aligned_pcs_in_its_bank() {
    let catalog = BankWordCatalog::new(BASE, &[0x2402_0001, 0x0100_0008, 0x7801_2345]);
    assert_eq!(catalog.len(), 3);
    assert_eq!(catalog.kind_at(BASE), Some(BankWordKind::Straight));
    assert_eq!(
        catalog.kind_at(BASE + 4),
        Some(BankWordKind::ControlTransfer)
    );
    assert_eq!(catalog.kind_at(BASE + 8), Some(BankWordKind::Unknown));
    assert_eq!(catalog.kind_at(BASE + 2), None);
    assert_eq!(catalog.kind_at(BASE - 4), None);
    assert_eq!(catalog.kind_at(BASE + 12), None);
    assert_eq!(catalog.kind_at_compact(BASE), Some(BankWordKind::Straight));
    assert_eq!(
        catalog.kind_at_compact(BASE + 4),
        Some(BankWordKind::ControlTransfer)
    );
    assert_eq!(catalog.kind_at_compact(BASE + 2), None);
    assert_eq!(catalog.runs().len(), 3);
}

fn current_rlib(deps: &Path) -> PathBuf {
    std::fs::read_dir(deps)
        .expect("read target deps directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("libfn64_recomp_rs-") && name.ends_with(".rlib")
                })
        })
        .max_by_key(|path| path.metadata().and_then(|meta| meta.modified()).ok())
        .expect("fn64_recomp_rs rlib beside integration test")
}

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
    let emitted_leaf_function = fn64_recomp_rs::emit_function(&fn64_recomp_rs::FuncInput {
        name: "run_leaf_function",
        vram: 0x8000_3000,
        words: &leaf_words,
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
use fn64_recomp_rs::{{
    BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CodeSpan, CpuFault,
    CpuFaultKind, ExecutionKey, GeneratedBankRunner, GuestPc, InstructionBudget,
    ProgramError, Rdram, RecompContext,
}};

{emitted}
{emitted_leaf_bank}
{emitted_leaf_function}
{emitted_sparse_bank}

fn main() {{
    let mut storage = vec![0u8; 64];
    let mut mem = Rdram::new(&mut storage);

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
        BlockExit::Fault(CpuFault {{ kind: CpuFaultKind::UnalignedPc, .. }})
    ));
    assert_eq!(unaligned.instructions, 0);

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
    let rlib = current_rlib(&deps);
    let compile = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("--edition=2021")
        .arg(&source_path)
        .arg("--extern")
        .arg(format!("fn64_recomp_rs={}", rlib.display()))
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

#[test]
fn legacy_function_runner_snapshots_computed_jr_before_delay_slot() {
    let emitted = fn64_recomp_rs::emit_function(&fn64_recomp_rs::FuncInput {
        name: "jr_snapshot",
        vram: BASE,
        words: &[0x0100_0008, 0x2408_1234],
    });
    let snapshot = emitted
        .find("let _target = ctx.r_u32(8);")
        .expect("computed JR target snapshot");
    let delay_write = emitted
        .find("ctx.set_r32(8, (0i32).wrapping_add(4660));")
        .expect("JR delay-slot write");
    assert!(
        snapshot < delay_write,
        "JR target must be captured before its delay slot:\n{emitted}"
    );
}

/// Compile a generated bank runner plus a `main` body into a host binary and
/// run it, asserting a clean exit. Returns the harness stdout. Mirrors the
/// infrastructure in [`emitted_bank_runner_compiles_and_executes_from_arbitrary_pcs`]
/// so the memory-fault probes execute real generated code rather than matching
/// on emitted text.
fn compile_and_run(emitted: &str, main_body: &str) -> String {
    let source = format!(
        r#"#![allow(unused_imports)]
use fn64_recomp_rs::{{
    BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CodeSpan, CpuFault,
    CpuFaultKind, ExecutionKey, GeneratedBankRunner, GuestPc, InstructionBudget,
    ProgramError, Rdram, RecompContext,
}};

{emitted}

fn main() {{
{main_body}
}}
"#
    );

    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let stamp = format!("{:?}", std::time::SystemTime::now());
    let key: String = stamp.chars().filter(char::is_ascii_alphanumeric).collect();
    let source_path = out_dir.join(format!("fn64_mem_fault_gate_{key}.rs"));
    let binary_path = out_dir.join(format!("fn64_mem_fault_gate_{key}"));
    std::fs::write(&source_path, source).expect("write generated fault-gate source");

    let deps = std::env::current_exe()
        .expect("current integration-test executable")
        .parent()
        .expect("target deps directory")
        .to_path_buf();
    let rlib = current_rlib(&deps);
    let compile = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("--edition=2021")
        .arg(&source_path)
        .arg("--extern")
        .arg(format!("fn64_recomp_rs={}", rlib.display()))
        .arg("-L")
        .arg(format!("dependency={}", deps.display()))
        .arg("-o")
        .arg(&binary_path)
        .output()
        .expect("invoke rustc for generated fault gate");
    assert!(
        compile.status.success(),
        "generated fault gate did not compile:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&binary_path)
        .output()
        .expect("run generated fault gate");
    assert!(
        run.status.success(),
        "generated fault gate failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).into_owned()
}

// A synthetic bank whose entry stub loads a high base ($t0 = 0xFFFFFFFF_80000000
// via LUI) and then touches offset 0x40 into it — guest VA 0xFFFFFFFF_80000040,
// physical byte 0x40. Run against an Rdram far smaller than 0x40 bytes, that
// access is outside backed storage: the block lane must raise a typed
// MemoryFault instead of the host slice-range panic the function lane keeps.
const FAULT_BASE: u32 = 0x8000_1000;
// Effective address of the faulting access: LUI $t0,0x8000 leaves $t0 =
// sign-extended 0xFFFFFFFF_80000000, plus the +0x40 store/load offset.
const FAULT_ADDR: u64 = 0xFFFF_FFFF_8000_0040;

#[test]
fn bank_store_outside_backed_rdram_is_a_typed_memory_fault() {
    // lui $t0,0x8000 ; sw $v0,0x40($t0) ; jr $ra ; nop
    let words = [0x3C08_8000u32, 0xAD02_0040, 0x03E0_0008, 0x0000_0000];
    let emitted = emit_bank_runner(&BankInput {
        name: "store_fault_bank",
        bank: BankId::new(0x51),
        vram: FAULT_BASE,
        words: &words,
    });
    let stdout = compile_and_run(
        &emitted,
        &format!(
            r#"    let mut storage = vec![0u8; 16];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    let run = store_fault_bank(
        ExecutionKey::new(BankId::new(0x51), GuestPc::new({FAULT_BASE:#010X})),
        InstructionBudget::new(64).unwrap(),
        &mut ctx,
        &mut mem,
    );
    // The LUI retired; the faulting SW did not. Excludes the faulting instruction.
    assert_eq!(run.instructions, 1, "expected only the LUI to retire: {{run:?}}");
    match run.exit {{
        BlockExit::Fault(CpuFault {{ at, kind: CpuFaultKind::MemoryFault {{ addr }} }}) => {{
            assert_eq!(at, ExecutionKey::new(BankId::new(0x51), GuestPc::new({store_pc:#010X})));
            assert_eq!(addr, {FAULT_ADDR:#018X}u64);
            println!("store fault at pc={{}} addr={{addr:#018X}}", at.pc);
        }}
        other => panic!("expected typed MemoryFault, got {{other:?}}"),
    }}"#,
            store_pc = FAULT_BASE + 4,
        ),
    );
    assert!(stdout.contains("store fault"), "{stdout}");
}

#[test]
fn bank_load_outside_backed_rdram_is_a_typed_memory_fault() {
    // lui $t0,0x8000 ; lw $v0,0x40($t0) ; jr $ra ; nop
    let words = [0x3C08_8000u32, 0x8D02_0040, 0x03E0_0008, 0x0000_0000];
    let emitted = emit_bank_runner(&BankInput {
        name: "load_fault_bank",
        bank: BankId::new(0x52),
        vram: FAULT_BASE,
        words: &words,
    });
    let stdout = compile_and_run(
        &emitted,
        &format!(
            r#"    let mut storage = vec![0u8; 16];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    let run = load_fault_bank(
        ExecutionKey::new(BankId::new(0x52), GuestPc::new({FAULT_BASE:#010X})),
        InstructionBudget::new(64).unwrap(),
        &mut ctx,
        &mut mem,
    );
    assert_eq!(run.instructions, 1, "expected only the LUI to retire: {{run:?}}");
    match run.exit {{
        BlockExit::Fault(CpuFault {{ at, kind: CpuFaultKind::MemoryFault {{ addr }} }}) => {{
            assert_eq!(at, ExecutionKey::new(BankId::new(0x52), GuestPc::new({load_pc:#010X})));
            assert_eq!(addr, {FAULT_ADDR:#018X}u64);
            // The destination register keeps its old value; the load never landed.
            assert_eq!(ctx.r_u32(2), 0);
            println!("load fault at pc={{}} addr={{addr:#018X}}", at.pc);
        }}
        other => panic!("expected typed MemoryFault, got {{other:?}}"),
    }}"#,
            load_pc = FAULT_BASE + 4,
        ),
    );
    assert!(stdout.contains("load fault"), "{stdout}");
}

#[test]
fn bank_fault_in_a_branch_delay_slot_does_not_commit_the_branch() {
    // lui $t0,0x8000 ; beq $zero,$zero,-2 ; sw $v0,0x40($t0) (delay) ; jr $ra ; nop
    //
    // The BEQ target is BASE (a valid in-bank arm), but its delay-slot store
    // faults. Architecturally a delay-slot exception annuls the branch: the
    // runner must return the typed fault, NOT a Transfer to the branch target,
    // and count only the instructions that retired before the branch (the LUI).
    let words = [
        0x3C08_8000u32, // lui $t0,0x8000
        0x1000_FFFE,    // beq $zero,$zero,-2  (target = BASE)
        0xAD02_0040,    // sw  $v0,0x40($t0)   (delay slot: faults)
        0x03E0_0008,    // jr  $ra
        0x0000_0000,    // nop
    ];
    let emitted = emit_bank_runner(&BankInput {
        name: "delay_fault_bank",
        bank: BankId::new(0x53),
        vram: FAULT_BASE,
        words: &words,
    });
    let stdout = compile_and_run(
        &emitted,
        &format!(
            r#"    let mut storage = vec![0u8; 16];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    let run = delay_fault_bank(
        ExecutionKey::new(BankId::new(0x53), GuestPc::new({FAULT_BASE:#010X})),
        InstructionBudget::new(64).unwrap(),
        &mut ctx,
        &mut mem,
    );
    // Only the LUI retired; the branch is annulled by the delay-slot fault.
    assert_eq!(run.instructions, 1, "delay-slot fault must not retire the branch/delay pair: {{run:?}}");
    match run.exit {{
        BlockExit::Fault(CpuFault {{ at, kind: CpuFaultKind::MemoryFault {{ addr }} }}) => {{
            assert_eq!(at, ExecutionKey::new(BankId::new(0x53), GuestPc::new({delay_pc:#010X})));
            assert_eq!(addr, {FAULT_ADDR:#018X}u64);
            println!("delay-slot fault at pc={{}} addr={{addr:#018X}}, branch not committed", at.pc);
        }}
        BlockExit::Transfer(_) => panic!("branch committed despite delay-slot fault: {{run:?}}"),
        other => panic!("expected typed MemoryFault, got {{other:?}}"),
    }}"#,
            delay_pc = FAULT_BASE + 8,
        ),
    );
    assert!(stdout.contains("branch not committed"), "{stdout}");
}

#[test]
fn bank_ending_inside_a_delay_slot_is_rejected_loudly() {
    let result = std::panic::catch_unwind(|| {
        emit_bank_runner(&BankInput {
            name: "truncated",
            bank: BankId::new(0xCC),
            vram: BASE,
            words: &[0x03E0_0008],
        })
    });
    let panic = result.expect_err("truncated delay slot must not be emitted");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic");
    assert!(message.contains("omits its delay slot"), "{message}");
}
