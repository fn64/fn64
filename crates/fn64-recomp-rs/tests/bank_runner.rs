//! Compile-and-run gate for bank-qualified arbitrary-PC emission.

use std::path::PathBuf;
use std::process::Command;

mod support;
use support::dev_interpreter_rlib;

use fn64_recomp_rs::{BankId, BankWordKind};
use fn64_recomp_rs_codegen::{
    classify_bank_words, emit_bank_runner, emit_bank_runner_with_host_calls,
    emit_dense_bank_shard_runner_function, emit_sparse_bank_runner, BankBlockInput, BankInput,
    BankWordCatalog, DenseBankShardInput, DenseEmitError, SparseBankInput,
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

#[test]
fn every_naturally_aligned_memory_family_emits_an_address_fault_check() {
    let i_type = |op: u32| (op << 26) | (4 << 21) | (2 << 16) | 1;
    let checked_loads = [0x21, 0x25, 0x23, 0x27, 0x30, 0x31, 0x37, 0x34, 0x35];
    let checked_stores = [0x29, 0x2B, 0x38, 0x39, 0x3F, 0x3C, 0x3D];
    let intentionally_unaligned = [
        0x20, 0x24, // LB/LBU
        0x22, 0x26, 0x1A, 0x1B, // LWL/LWR/LDL/LDR
        0x28, // SB
        0x2A, 0x2E, 0x2C, 0x2D, // SWL/SWR/SDL/SDR
    ];
    let words = checked_loads
        .iter()
        .chain(&checked_stores)
        .chain(&intentionally_unaligned)
        .copied()
        .map(i_type)
        .collect::<Vec<_>>();
    let emitted = emit_bank_runner(&BankInput {
        name: "run_memory_alignment_audit",
        bank: BankId::new(0xAB),
        vram: 0x8000_C000,
        words: &words,
    });

    assert_eq!(
        emitted
            .matches("address_error(FaultSite::straight(expected_bank,")
            .count(),
        checked_loads.len() + checked_stores.len()
    );
    assert_eq!(
        emitted.matches("DataAccessKind::Load").count(),
        checked_loads.len()
    );
    assert_eq!(
        emitted.matches("DataAccessKind::Store").count(),
        checked_stores.len()
    );
    assert_eq!(
        emitted.matches("let effective_address =").count(),
        checked_loads.len() + checked_stores.len()
    );
}

/// Compile one emitted runner in an isolated process and return its stdout.
///
/// The process id separates concurrent test binaries; the monotonic key
/// separates parallel tests inside this binary even when their clocks have the
/// same resolution. Generated files therefore never overwrite one another.
fn compile_and_run(emitted: &str, main_body: &str) -> String {
    let source = format!(
        r#"#![allow(unused_imports)]
use fn64_recomp_rs::{{
    finalize_executable_write_exit, run_bank, set_guest_write_boundary_observer,
    take_executable_write_boundary, BankId, BlockExit, BlockProgram, BlockRun, CodeBank,
    CodeCatalog, CodeSpan, CpuException, CpuFault, CpuFaultKind, ExecutionKey,
    GeneratedBankRunner, GuestPc, GuestWriteBoundary, GuestWriteEvent, InstructionBudget,
    ProgramError, Rdram, RecompContext, TlbEntryRaw,
}};

{emitted}

fn main() {{
{main_body}
}}
"#
    );

    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let key = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let process = std::process::id();
    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let source_path = out_dir.join(format!("fn64_bank_runner_{process}_{key}.rs"));
    let binary_path = out_dir.join(format!("fn64_bank_runner_{process}_{key}"));
    std::fs::write(&source_path, source).expect("write isolated bank-runner source");

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
        .arg(format!("fn64_recomp_rs={}", rlib.display()))
        .arg("-L")
        .arg(format!("dependency={}", deps.display()))
        .arg("-o")
        .arg(&binary_path)
        .output()
        .expect("invoke rustc for isolated bank runner");
    assert!(
        compile.status.success(),
        "isolated bank runner did not compile:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&binary_path)
        .output()
        .expect("run isolated bank runner");
    assert!(
        run.status.success(),
        "isolated bank runner failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).into_owned()
}

#[test]
fn contiguous_and_sparse_runners_stop_after_executable_store() {
    let words = [
        0xac88_0000, // sw    $t0,0($a0)
        0x2402_0001, // addiu $v0,$zero,1 -- stale sentinel
        0x03e0_0008, // jr    $ra
        0x0000_0000, // nop
    ];
    let contiguous_bank = BankId::new(0xC100);
    let sparse_bank = BankId::new(0xC200);
    let contiguous = emit_bank_runner(&BankInput {
        name: "run_exec_write_contiguous",
        bank: contiguous_bank,
        vram: BASE,
        words: &words,
    });
    let sparse_base = BASE + 0x100;
    let sparse = emit_sparse_bank_runner(&SparseBankInput {
        name: "run_exec_write_sparse",
        bank: sparse_bank,
        blocks: &[BankBlockInput {
            vram: sparse_base,
            words: &words,
        }],
    });
    let emitted = format!("{contiguous}\n{sparse}");
    compile_and_run(
        &emitted,
        &format!(
            r#"
fn executable_boundary(event: GuestWriteEvent) -> GuestWriteBoundary {{
    let (start, len) = event.range();
    let end = start + len;
    if start < 0x24 && end > 0x20 {{
        GuestWriteBoundary::ExecutableChanged
    }} else {{
        GuestWriteBoundary::Continue
    }}
}}

let mut bytes = vec![0u8; 0x100];
let mut mem = Rdram::new(&mut bytes);
set_guest_write_boundary_observer(Some(executable_boundary));

let mut contiguous_ctx = RecompContext::new();
contiguous_ctx.set_r(4, 0xffff_ffff_8000_0020);
contiguous_ctx.set_r(8, 0x1122_3344);
contiguous_ctx.set_r(31, 0x8000_9000);
let stopped = run_exec_write_contiguous(
    ExecutionKey::new(BankId::new({}), GuestPc::new({BASE:#010x})),
    InstructionBudget::new(8).unwrap(),
    &mut contiguous_ctx,
    &mut mem,
);
assert_eq!(stopped.instructions, 1);
assert_eq!(contiguous_ctx.r(2), 0);
assert_eq!(mem.load_w(0xffff_ffff_8000_0020) as u32, 0x1122_3344);
assert_eq!(stopped.exit, BlockExit::ExecutableWrite {{
    source_bank: BankId::new({}),
    resume: ExecutionKey::new(BankId::new({}), GuestPc::new({:#010x})),
}});

// A non-overlapping store remains ordinary and the stale sentinel therefore
// executes in the same runner turn.
let mut ordinary_ctx = RecompContext::new();
ordinary_ctx.set_r(4, 0xffff_ffff_8000_0010);
ordinary_ctx.set_r(8, 0x5566_7788);
ordinary_ctx.set_r(31, 0x8000_9000);
let ordinary = run_exec_write_contiguous(
    ExecutionKey::new(BankId::new({}), GuestPc::new({BASE:#010x})),
    InstructionBudget::new(8).unwrap(),
    &mut ordinary_ctx,
    &mut mem,
);
assert_eq!(ordinary_ctx.r(2), 1);
assert!(!matches!(ordinary.exit, BlockExit::ExecutableWrite {{ .. }}));

let mut sparse_ctx = RecompContext::new();
sparse_ctx.set_r(4, 0xffff_ffff_8000_0020);
sparse_ctx.set_r(8, 0x99aa_bbcc);
sparse_ctx.set_r(31, 0x8000_9000);
let sparse_stopped = run_exec_write_sparse(
    ExecutionKey::new(BankId::new({}), GuestPc::new({sparse_base:#010x})),
    InstructionBudget::new(8).unwrap(),
    &mut sparse_ctx,
    &mut mem,
);
assert_eq!(sparse_stopped.instructions, 1);
assert_eq!(sparse_ctx.r(2), 0);
assert_eq!(sparse_stopped.exit, BlockExit::ExecutableWrite {{
    source_bank: BankId::new({}),
    resume: ExecutionKey::new(BankId::new({}), GuestPc::new({:#010x})),
}});
set_guest_write_boundary_observer(None);
"#,
            contiguous_bank.get(),
            contiguous_bank.get(),
            contiguous_bank.get(),
            BASE + 4,
            contiguous_bank.get(),
            sparse_bank.get(),
            sparse_bank.get(),
            sparse_bank.get(),
            sparse_base + 4,
        ),
    );
}

#[test]
fn emitted_and_interpreted_delay_slot_store_choose_the_same_boundary() {
    let words = [
        0x1000_0002, // beq   $zero,$zero,+2
        0xac88_0000, // sw    $t0,0($a0) -- delay slot
        0x2402_0001, // addiu $v0,$zero,1 -- fallthrough sentinel
        0x2403_0002, // addiu $v1,$zero,2 -- selected target sentinel
    ];
    let bank = BankId::new(0xC300);
    let emitted = emit_bank_runner(&BankInput {
        name: "run_exec_write_delay",
        bank,
        vram: BASE,
        words: &words,
    });
    compile_and_run(
        &emitted,
        &format!(
            r#"
fn executable_boundary(event: GuestWriteEvent) -> GuestWriteBoundary {{
    let (start, len) = event.range();
    if start < 0x24 && start + len > 0x20 {{
        GuestWriteBoundary::ExecutableChanged
    }} else {{
        GuestWriteBoundary::Continue
    }}
}}

let mut aot_bytes = vec![0u8; 0x100];
let mut aot_mem = Rdram::new(&mut aot_bytes);
let mut aot_ctx = RecompContext::new();
aot_ctx.set_r(4, 0xffff_ffff_8000_0020);
aot_ctx.set_r(8, 0x1122_3344);
set_guest_write_boundary_observer(Some(executable_boundary));
let aot = run_exec_write_delay(
    ExecutionKey::new(BankId::new({}), GuestPc::new({BASE:#010x})),
    InstructionBudget::new(8).unwrap(),
    &mut aot_ctx,
    &mut aot_mem,
);

let mut catalog = CodeCatalog::new();
catalog.register(CodeBank::new(
    BankId::new({}),
    GuestPc::new({BASE:#010x}),
    vec!{words:?},
).unwrap()).unwrap();
let mut interp_bytes = vec![0u8; 0x100];
let mut interp_mem = Rdram::new(&mut interp_bytes);
let mut interp_ctx = RecompContext::new();
interp_ctx.set_r(4, 0xffff_ffff_8000_0020);
interp_ctx.set_r(8, 0x1122_3344);
set_guest_write_boundary_observer(Some(executable_boundary));
let interpreted = run_bank(
    &catalog,
    BankId::new({}),
    ExecutionKey::new(BankId::new({}), GuestPc::new({BASE:#010x})),
    InstructionBudget::new(8).unwrap(),
    &mut interp_ctx,
    &mut interp_mem,
).unwrap();

assert_eq!(aot.instructions, 2);
assert_eq!(aot, interpreted);
assert_eq!(aot_ctx.r(2), interp_ctx.r(2));
assert_eq!(aot_ctx.r(3), interp_ctx.r(3));
assert_eq!(aot_ctx.r(8), interp_ctx.r(8));
assert_eq!(aot_mem.load_w(0xffff_ffff_8000_0020), interp_mem.load_w(0xffff_ffff_8000_0020));
assert_eq!(aot_ctx.r(2), 0);
assert_eq!(aot_ctx.r(3), 0);
assert_eq!(aot.exit, BlockExit::ExecutableWrite {{
    source_bank: BankId::new({}),
    resume: ExecutionKey::new(BankId::new({}), GuestPc::new({:#010x})),
}});
set_guest_write_boundary_observer(None);
"#,
            bank.get(),
            bank.get(),
            bank.get(),
            bank.get(),
            bank.get(),
            bank.get(),
            BASE + 12,
        ),
    );
}

#[test]
fn executable_delay_store_and_unaligned_runtime_transfer_match_across_lanes() {
    let words = [
        0x0120_0008, // jr    $t1
        0xac88_0000, // sw    $t0,0($a0) -- delay slot
        0x0120_8009, // jalr  $s0,$t1
        0xac88_0000, // sw    $t0,0($a0) -- delay slot
    ];
    let bank = BankId::new(0xC301);
    let emitted = emit_bank_runner(&BankInput {
        name: "run_exec_write_unaligned",
        bank,
        vram: BASE,
        words: &words,
    });
    compile_and_run(
        &emitted,
        &format!(
            r#"
fn executable_boundary(event: GuestWriteEvent) -> GuestWriteBoundary {{
    let (start, len) = event.range();
    if start < 0x24 && start + len > 0x20 {{
        GuestWriteBoundary::ExecutableChanged
    }} else {{
        GuestWriteBoundary::Continue
    }}
}}

let words = vec!{words:?};
for entry_offset in [0u32, 8] {{
    for budget in [2u32, 3] {{
        let mut aot_bytes = vec![0u8; 0x100];
        let mut aot_mem = Rdram::new(&mut aot_bytes);
        let mut aot_ctx = RecompContext::new();
        aot_ctx.set_r(4, 0xffff_ffff_8000_0020);
        aot_ctx.set_r(8, 0x1122_3344);
        aot_ctx.set_r(9, 0x8000_2002);
        set_guest_write_boundary_observer(Some(executable_boundary));
        let aot = run_exec_write_unaligned(
            ExecutionKey::new(BankId::new({}), GuestPc::new({BASE:#010x} + entry_offset)),
            InstructionBudget::new(budget).unwrap(),
            &mut aot_ctx,
            &mut aot_mem,
        );
        assert!(!take_executable_write_boundary());

        let mut catalog = CodeCatalog::new();
        catalog.register(CodeBank::new(
            BankId::new({}),
            GuestPc::new({BASE:#010x}),
            words.clone(),
        ).unwrap()).unwrap();
        let mut interp_bytes = vec![0u8; 0x100];
        let mut interp_mem = Rdram::new(&mut interp_bytes);
        let mut interp_ctx = RecompContext::new();
        interp_ctx.set_r(4, 0xffff_ffff_8000_0020);
        interp_ctx.set_r(8, 0x1122_3344);
        interp_ctx.set_r(9, 0x8000_2002);
        set_guest_write_boundary_observer(Some(executable_boundary));
        let interpreted = run_bank(
            &catalog,
            BankId::new({}),
            ExecutionKey::new(BankId::new({}), GuestPc::new({BASE:#010x} + entry_offset)),
            InstructionBudget::new(budget).unwrap(),
            &mut interp_ctx,
            &mut interp_mem,
        ).unwrap();
        assert!(!take_executable_write_boundary());

        assert_eq!(aot, interpreted, "entry offset {{entry_offset}}, budget {{budget}}");
        assert_eq!(aot_ctx.r(16), interp_ctx.r(16));
        assert_eq!(aot_mem.load_w(0xffff_ffff_8000_0020), interp_mem.load_w(0xffff_ffff_8000_0020));
        let target = ExecutionKey::new(BankId::new({}), GuestPc::new(0x8000_2002));
        if budget == 2 {{
            assert_eq!(aot, BlockRun::new(BlockExit::Checkpoint(target), 2));
        }} else {{
            assert_eq!(aot.instructions, 3);
            assert_eq!(aot.exit, BlockExit::ExecutableWriteFault(
                CpuFault::instruction_address_error(target),
            ));
        }}
        if entry_offset == 8 {{
            assert_eq!(aot_ctx.r_u32(16), {:#010x});
        }}
    }}
}}
set_guest_write_boundary_observer(None);
"#,
            bank.get(),
            bank.get(),
            bank.get(),
            bank.get(),
            bank.get(),
            BASE + 16,
        ),
    );
}

#[test]
fn emitted_and_interpreted_tlb_translation_and_faults_are_identical() {
    let words = [
        0x8c82_0000, // lw $v0,0($a0)
        0xac83_0004, // sw $v1,4($a0)
        0x03e0_0008, // jr $ra
        0,
        0x1000_0001, // beq $zero,$zero,+1
        0x8c82_0000, // lw $v0,0($a0) -- delay slot
        0x03e0_0008, // jr $ra
        0,
    ];
    let bank = BankId::new(0xA5);
    let emitted = emit_bank_runner(&BankInput {
        name: "run_tlb_data_bank",
        bank,
        vram: BASE,
        words: &words,
    });
    let stdout = compile_and_run(
        &emitted,
        r#"
    const WORDS: [u32; 8] = [
        0x8c82_0000, 0xac83_0004, 0x03e0_0008, 0,
        0x1000_0001, 0x8c82_0000, 0x03e0_0008, 0,
    ];
    let bank = BankId::new(0xA5);
    let code = CodeBank::new(bank, GuestPc::new(0x8000_1000), WORDS.to_vec()).unwrap();
    let mut catalog = CodeCatalog::new();
    catalog.register(code).unwrap();
    let budget = InstructionBudget::new(8).unwrap();

    let make_ctx = |entry_lo0: u32, install: bool| {
        let mut ctx = RecompContext::new();
        ctx.set_r(4, 0x0040_0000);
        ctx.set_r(3, 0xa1b2_c3d4);
        ctx.set_r(31, 0x8000_9000);
        ctx.cop0_entry_hi = 0x0040_002a;
        if install {
            ctx.tlb_entries[7] = TlbEntryRaw {
                page_mask: 0,
                entry_hi: 0x0040_002a,
                entry_lo0,
                entry_lo1: (1 << 6) | 0x6,
            };
        }
        ctx
    };
    let compare = |entry: u32, entry_lo0: u32, install: bool, initial: [u8; 16]| {
        let key = ExecutionKey::new(bank, GuestPc::new(entry));
        let mut ictx = make_ctx(entry_lo0, install);
        let mut actx = make_ctx(entry_lo0, install);
        let mut imem = initial;
        let mut amem = initial;
        let irun = run_bank(
            &catalog,
            bank,
            key,
            budget,
            &mut ictx,
            &mut Rdram::new(&mut imem),
        ).unwrap();
        let arun = run_tlb_data_bank(
            key,
            budget,
            &mut actx,
            &mut Rdram::new(&mut amem),
        );
        assert_eq!(irun, arun);
        assert_eq!(ictx.gprs(), actx.gprs());
        assert_eq!(imem, amem);
        (arun, actx, amem)
    };

    let mut initial = [0u8; 16];
    initial[0..4].copy_from_slice(&0x1122_3344u32.to_ne_bytes());
    let (valid, valid_ctx, valid_mem) = compare(0x8000_1000, 0x6, true, initial);
    assert_eq!(valid.instructions, 4);
    assert_eq!(valid_ctx.r_u32(2), 0x1122_3344);
    assert_eq!(&valid_mem[4..8], &0xa1b2_c3d4u32.to_ne_bytes());

    let (refill, _, _) = compare(0x8000_1000, 0, false, initial);
    assert!(matches!(refill.exit, BlockExit::Fault(CpuFault {
        kind: CpuFaultKind::Exception {
            exception: CpuException::TlbRefillLoad,
            bad_vaddr: Some(0x0040_0000),
            ..
        }, ..
    })));
    assert_eq!(refill.instructions, 1);

    let (invalid, _, _) = compare(0x8000_1000, 0, true, initial);
    assert!(matches!(invalid.exit, BlockExit::Fault(CpuFault {
        kind: CpuFaultKind::Exception {
            exception: CpuException::TlbInvalidLoad,
            ..
        }, ..
    })));

    let (modified, _, unchanged) = compare(0x8000_1004, 0x2, true, initial);
    assert!(matches!(modified.exit, BlockExit::Fault(CpuFault {
        kind: CpuFaultKind::Exception {
            exception: CpuException::TlbModified,
            ..
        }, ..
    })));
    assert_eq!(modified.instructions, 1);
    assert_eq!(unchanged, initial);

    let (delay_refill, _, _) = compare(0x8000_1010, 0, false, initial);
    assert!(matches!(delay_refill.exit, BlockExit::Fault(CpuFault {
        at: ExecutionKey { pc, .. },
        kind: CpuFaultKind::Exception {
            exception: CpuException::TlbRefillLoad,
            epc,
            branch_delay: true,
            ..
        },
    }) if pc == GuestPc::new(0x8000_1014) && epc == GuestPc::new(0x8000_1010)));
    assert_eq!(delay_refill.instructions, 2);
    println!("tlb-data-differential-ok");
"#,
    );
    assert_eq!(stdout.trim(), "tlb-data-differential-ok");
}

#[test]
fn emitted_and_interpreted_extended_address_spaces_are_identical() {
    let words = [
        0x8c82_0000, // lw $v0,0($a0)
        0xac83_0004, // sw $v1,4($a0)
        0x03e0_0008, // jr $ra
        0,
    ];
    let bank = BankId::new(0xA6);
    let emitted = emit_bank_runner(&BankInput {
        name: "run_extended_data_bank",
        bank,
        vram: BASE,
        words: &words,
    });
    let stdout = compile_and_run(
        &emitted,
        r#"
    const WORDS: [u32; 4] = [0x8c82_0000, 0xac83_0004, 0x03e0_0008, 0];
    const MAPPED: u64 = 0x0000_0012_3456_0000;
    const XKPHYS: u64 = 0x9000_0000_0000_0000;
    let bank = BankId::new(0xA6);
    let code = CodeBank::new(bank, GuestPc::new(0x8000_1000), WORDS.to_vec()).unwrap();
    let mut catalog = CodeCatalog::new();
    catalog.register(code).unwrap();
    let budget = InstructionBudget::new(8).unwrap();

    let make_ctx = |status: u32, address: u64, install: bool| {
        let mut ctx = RecompContext::new();
        ctx.cop0_status = status;
        ctx.cop0_entry_hi = 0x2a;
        ctx.set_r(4, address);
        ctx.set_r(3, 0xa1b2_c3d4);
        ctx.set_r(31, 0x8000_9000);
        if install {
            ctx.tlb_entries[7] = TlbEntryRaw {
                page_mask: 0,
                entry_hi: (address & 0xc000_00ff_ffff_e000) | 0x2a,
                entry_lo0: 0x6,
                entry_lo1: (1 << 6) | 0x6,
            };
        }
        ctx
    };
    let compare = |status: u32, address: u64, install: bool, initial: [u8; 16]| {
        let key = ExecutionKey::new(bank, GuestPc::new(0x8000_1000));
        let mut ictx = make_ctx(status, address, install);
        let mut actx = make_ctx(status, address, install);
        let mut imem = initial;
        let mut amem = initial;
        let irun = run_bank(
            &catalog,
            bank,
            key,
            budget,
            &mut ictx,
            &mut Rdram::new(&mut imem),
        ).unwrap();
        let arun = run_extended_data_bank(
            key,
            budget,
            &mut actx,
            &mut Rdram::new(&mut amem),
        );
        assert_eq!(irun, arun);
        assert_eq!(ictx.gprs(), actx.gprs());
        assert_eq!(imem, amem);
        (arun, actx, amem)
    };

    let mut initial = [0u8; 16];
    initial[0..4].copy_from_slice(&0x1122_3344u32.to_ne_bytes());

    // User mode plus UX selects mapped XUSEG and compares the full VPN2 tag.
    let (mapped, mapped_ctx, mapped_mem) = compare((2 << 3) | (1 << 5), MAPPED, true, initial);
    assert_eq!(mapped.instructions, 4);
    assert_eq!(mapped_ctx.r_u32(2), 0x1122_3344);
    assert_eq!(&mapped_mem[4..8], &0xa1b2_c3d4u32.to_ne_bytes());

    let (refill, _, _) = compare((2 << 3) | (1 << 5), MAPPED, false, initial);
    assert!(matches!(refill.exit, BlockExit::Fault(CpuFault {
        kind: CpuFaultKind::Exception {
            exception: CpuException::XTlbRefillLoad,
            bad_vaddr: Some(MAPPED),
            ..
        }, ..
    })));
    assert_eq!(refill.instructions, 1);

    // Kernel KX reaches valid XKPHYS directly; the same VA is illegal in user mode.
    let (direct, direct_ctx, direct_mem) = compare(1 << 7, XKPHYS, false, initial);
    assert_eq!(direct.instructions, 4);
    assert_eq!(direct_ctx.r_u32(2), 0x1122_3344);
    assert_eq!(&direct_mem[4..8], &0xa1b2_c3d4u32.to_ne_bytes());

    let (address_error, _, unchanged) = compare((2 << 3) | (1 << 5), XKPHYS, false, initial);
    assert!(matches!(address_error.exit, BlockExit::Fault(CpuFault {
        kind: CpuFaultKind::Exception {
            exception: CpuException::AddressErrorLoad,
            bad_vaddr: Some(XKPHYS),
            ..
        }, ..
    })));
    assert_eq!(address_error.instructions, 1);
    assert_eq!(unchanged, initial);
    println!("extended-data-differential-ok");
"#,
    );
    assert_eq!(stdout.trim(), "extended-data-differential-ok");
}

#[test]
fn emitted_and_interpreted_doubleword_translation_register_moves_are_identical() {
    let words = [
        0x40a4_5000, // dmtc0 $a0,EntryHi
        0x4022_5000, // dmfc0 $v0,EntryHi
        0x40a5_a000, // dmtc0 $a1,XContext
        0x4023_a000, // dmfc0 $v1,XContext
        0x03e0_0008, // jr $ra
        0,
    ];
    let bank = BankId::new(0xA7);
    let emitted = emit_bank_runner(&BankInput {
        name: "run_doubleword_cop0_bank",
        bank,
        vram: BASE,
        words: &words,
    });
    let stdout = compile_and_run(
        &emitted,
        r#"
    const WORDS: [u32; 6] = [
        0x40a4_5000, 0x4022_5000, 0x40a5_a000,
        0x4023_a000, 0x03e0_0008, 0,
    ];
    let bank = BankId::new(0xA7);
    let code = CodeBank::new(bank, GuestPc::new(0x8000_1000), WORDS.to_vec()).unwrap();
    let mut catalog = CodeCatalog::new();
    catalog.register(code).unwrap();
    let key = ExecutionKey::new(bank, GuestPc::new(0x8000_1000));
    let budget = InstructionBudget::new(8).unwrap();
    let mut ictx = RecompContext::new();
    let mut actx = RecompContext::new();
    for ctx in [&mut ictx, &mut actx] {
        ctx.set_r(4, 0x4000_0088_7654_205a);
        ctx.set_r(5, 0x1234_5678_9abc_def0);
        ctx.set_r(31, 0x8000_9000);
    }
    let mut imem = [];
    let mut amem = [];
    let irun = run_bank(
        &catalog,
        bank,
        key,
        budget,
        &mut ictx,
        &mut Rdram::new(&mut imem),
    ).unwrap();
    let arun = run_doubleword_cop0_bank(
        key,
        budget,
        &mut actx,
        &mut Rdram::new(&mut amem),
    );
    assert_eq!(irun, arun);
    assert_eq!(ictx.gprs(), actx.gprs());
    assert_eq!(ictx.cop0_entry_hi, 0x4000_0088_7654_205a);
    assert_eq!(ictx.cop0_xcontext, 0x1234_5678_9abc_def0);
    assert_eq!(ictx.r_u64(2), ictx.cop0_entry_hi);
    assert_eq!(ictx.r_u64(3), ictx.cop0_xcontext);
    println!("doubleword-cop0-differential-ok");
"#,
    );
    assert_eq!(stdout.trim(), "doubleword-cop0-differential-ok");
}

#[test]
fn emitted_random_and_tlbwr_use_explicit_instruction_order() {
    let words = [
        0x2402_001d, // addiu $v0,$zero,29
        0x4082_3000, // mtc0  $v0,Wired
        0x4200_0006, // tlbwr
        0x4003_0800, // mfc0  $v1,Random
        0x03e0_0008, // jr    $ra
        0x0000_0000, // nop
    ];
    let emitted = emit_bank_runner(&BankInput {
        name: "run_tlb_random_bank",
        bank: BankId::new(0xA4),
        vram: BASE,
        words: &words,
    });
    let stdout = compile_and_run(
        &emitted,
        r#"
    let mut bytes = vec![];
    let mut mem = Rdram::new(&mut bytes);
    let mut ctx = RecompContext::new();
    ctx.set_r(31, 0x8000_9000);
    ctx.cop0_entry_hi = 0x1234_500a;
    ctx.cop0_entry_lo0 = 0x46;
    ctx.cop0_entry_lo1 = 0x86;
    ctx.cop0_page_mask = 0x6000;
    let run = run_tlb_random_bank(
        ExecutionKey::new(BankId::new(0xA4), GuestPc::new(0x8000_1000)),
        InstructionBudget::new(6).unwrap(),
        &mut ctx,
        &mut mem,
    );
    assert_eq!(run.instructions, 6);
    assert_eq!(ctx.r_u32(3), 29);
    assert_eq!(ctx.tlb_entries[30].entry_hi, 0x1234_500a);
    assert_eq!(ctx.tlb_entries[30].entry_lo0, 0x46);
    assert_eq!(ctx.tlb_entries[30].entry_lo1, 0x86);
    assert_eq!(ctx.tlb_entries[30].page_mask, 0x6000);
    assert_eq!(ctx.read_cop0(1), 29);
    println!("tlb-random-ok");
"#,
    );
    assert_eq!(stdout.trim(), "tlb-random-ok");
}

#[test]
fn emitted_reserved_words_compile_and_raise_precise_ri() {
    let words = [
        0x4c00_0000, // reserved primary opcode 0x13
        0x1000_0001, // beq $zero,$zero,+1
        0x4c00_0000, // reserved delay-slot encoding
        0x0000_0000,
    ];
    let emitted = emit_bank_runner(&BankInput {
        name: "run_reserved_bank",
        bank: BankId::new(0xA6),
        vram: BASE,
        words: &words,
    });
    let stdout = compile_and_run(
        &emitted,
        r#"
    let mut bytes = vec![];
    let mut mem = Rdram::new(&mut bytes);

    let mut straight_ctx = RecompContext::new();
    let straight = run_reserved_bank(
        ExecutionKey::new(BankId::new(0xA6), GuestPc::new(0x8000_1000)),
        InstructionBudget::new(4).unwrap(),
        &mut straight_ctx,
        &mut mem,
    );
    assert_eq!(straight.instructions, 1);
    let BlockExit::Fault(straight_fault) = straight.exit else { panic!("expected RI fault") };
    assert!(matches!(straight_fault.kind, CpuFaultKind::Exception {
        exception: CpuException::ReservedInstruction,
        epc,
        branch_delay: false,
        ..
    } if epc == GuestPc::new(0x8000_1000)));
    assert_eq!(straight_fault.enter_exception(&mut straight_ctx), Some(GuestPc::new(0x8000_0180)));
    assert_eq!((straight_ctx.cop0_cause >> 2) & 0x1f, 10);

    let mut delay_ctx = RecompContext::new();
    let delay = run_reserved_bank(
        ExecutionKey::new(BankId::new(0xA6), GuestPc::new(0x8000_1004)),
        InstructionBudget::new(4).unwrap(),
        &mut delay_ctx,
        &mut mem,
    );
    assert_eq!(delay.instructions, 2);
    let BlockExit::Fault(delay_fault) = delay.exit else { panic!("expected delay RI fault") };
    assert!(matches!(delay_fault.kind, CpuFaultKind::Exception {
        exception: CpuException::ReservedInstruction,
        epc,
        branch_delay: true,
        ..
    } if epc == GuestPc::new(0x8000_1004)));
    assert_eq!(delay_fault.enter_exception(&mut delay_ctx), Some(GuestPc::new(0x8000_0180)));
    assert_eq!((delay_ctx.cop0_cause >> 2) & 0x1f, 10);
    assert_ne!(delay_ctx.cop0_cause & (1 << 31), 0);
    println!("reserved-instruction-ri-ok");
"#,
    );
    assert_eq!(stdout.trim(), "reserved-instruction-ri-ok");
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
    let emitted_leaf_function =
        fn64_recomp_rs_codegen::emit_function(&fn64_recomp_rs_codegen::FuncInput {
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
use fn64_recomp_rs::{{
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
fn in_bank_jalr_keeps_host_first_call_resolution() {
    let bank = BankId::new(0xBA11);
    let words = [
        0x0320_f809, // jalr $ra,$t9
        0x2404_0007, // addiu $a0,$zero,7 (delay)
        0x2405_0009, // resume sentinel
        0x0000_0000, // admitted in-bank target
    ];
    let emitted = emit_bank_runner(&BankInput {
        name: "run_in_bank_jalr",
        bank,
        vram: BASE,
        words: &words,
    });
    let stdout = compile_and_run(
        &emitted,
        &format!(
            r#"
let mut storage = vec![];
let mut mem = Rdram::new(&mut storage);
let mut ctx = RecompContext::new();
ctx.set_r32(25, {target:#010x}u32 as i32);
let run = run_in_bank_jalr(
    ExecutionKey::new(BankId::new({bank_id}), GuestPc::new({BASE:#010x})),
    InstructionBudget::new(4).unwrap(),
    &mut ctx,
    &mut mem,
);
assert_eq!(ctx.r_u32(4), 7, "JALR delay slot must execute");
assert_eq!(ctx.r_u32(31), {resume:#010x});
assert_eq!(run.instructions, 2);
assert_eq!(run.exit, BlockExit::ResolveCall {{
    source_bank: BankId::new({bank_id}),
    target_pc: GuestPc::new({target:#010x}),
    resume: ExecutionKey::new(BankId::new({bank_id}), GuestPc::new({resume:#010x})),
}});
println!("in-bank-jalr-resolve-call-ok");
"#,
            bank_id = bank.get(),
            target = BASE + 12,
            resume = BASE + 8,
        ),
    );
    assert_eq!(stdout.trim(), "in-bank-jalr-resolve-call-ok");
}

#[test]
fn legacy_function_runner_snapshots_computed_jr_before_delay_slot() {
    let emitted = fn64_recomp_rs_codegen::emit_function(&fn64_recomp_rs_codegen::FuncInput {
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

#[test]
fn dense_shard_control_at_owned_end_uses_lookahead() {
    let bank = BankId::new(0xD001);
    let emitted = emit_dense_bank_shard_runner_function(&DenseBankShardInput {
        name: "run_dense_boundary_control",
        bank,
        image_vram_start: BASE,
        image_vram_end: BASE + 0x20,
        artifact_vram_start: BASE,
        artifact_vram_end: BASE + 0x20,
        shard_vram_start: BASE,
        words: &[0x1000_0002],              // beq zero,zero -> BASE+0xc
        delay_lookahead: Some(0x2404_0007), // addiu a0,zero,7
        verify_live_words: true,
    })
    .unwrap();
    compile_and_run(
        &emitted,
        &format!(
            r#"
let mut storage = vec![0u8; 0x2000];
let mut mem = Rdram::new(&mut storage);
mem.store_w(0xffff_ffff_0000_0000 | {BASE:#010x}, 0x1000_0002);
mem.store_w(0xffff_ffff_0000_0000 | {:#010x}, 0x2404_0007);
let mut ctx = RecompContext::new();
let run = run_dense_boundary_control(
    ExecutionKey::new(BankId::new({}), GuestPc::new({BASE:#010x})),
    InstructionBudget::new(8).unwrap(),
    &mut ctx,
    &mut mem,
);
assert_eq!(run.instructions, 2);
assert_eq!(ctx.r(4), 7);
assert_eq!(run.exit, BlockExit::Transfer(ExecutionKey::new(
    BankId::new({}), GuestPc::new({:#010x}),
)));
"#,
            BASE + 4,
            bank.get(),
            bank.get(),
            BASE + 12,
        ),
    );
}

#[test]
fn dense_straight_post_steps_use_the_shared_ordered_boundary() {
    let emitted = emit_dense_bank_shard_runner_function(&DenseBankShardInput {
        name: "run_dense_shared_post_step",
        bank: BankId::new(0xD00A),
        image_vram_start: BASE,
        image_vram_end: BASE + 16,
        artifact_vram_start: BASE,
        artifact_vram_end: BASE + 16,
        shard_vram_start: BASE,
        words: &[0, 0, 0, 0],
        delay_lookahead: None,
        verify_live_words: false,
    })
    .unwrap();

    assert_eq!(
        emitted
            .matches("fn64_recomp_rs::post_straight_instruction_exit(")
            .count(),
        4,
        "every ordinary arm must use the shared post-step:\n{emitted}"
    );
    assert!(
        !emitted.contains("fn64_recomp_rs::take_executable_write_boundary()"),
        "generated arms must not reconstruct executable-write priority:\n{emitted}"
    );
    assert!(
        !emitted.contains("if executed >= budget.get()"),
        "generated arms must not reconstruct checkpoint priority:\n{emitted}"
    );
    assert_eq!(
        emitted.matches("BlockExit::ExecutableWrite {").count(),
        0,
        "the shared helper owns executable-write exit construction"
    );
}

#[test]
fn dense_shared_post_step_measurably_compacts_one_subrunner() {
    let words = vec![0u32; 512];
    let emitted = emit_dense_bank_shard_runner_function(&DenseBankShardInput {
        name: "run_dense_compaction_metric",
        bank: BankId::new(0xD00B),
        image_vram_start: BASE,
        image_vram_end: BASE + 2048,
        artifact_vram_start: BASE,
        artifact_vram_end: BASE + 2048,
        shard_vram_start: BASE,
        words: &words,
        delay_lookahead: None,
        verify_live_words: false,
    })
    .unwrap();

    let mut removed_inline_bytes = 0usize;
    for index in 0..words.len() {
        let next = BASE + (index as u32 + 1) * 4;
        let may_continue_locally = index + 1 < words.len();
        let compact = format!(
            "            if let Some(exit) = fn64_recomp_rs::post_straight_instruction_exit(expected_bank, GuestPc::new({next:#010X}), executed, budget, {may_continue_locally}) {{ finish!(exit); }}\n"
        );
        let mut inline = format!(
            "            if fn64_recomp_rs::take_executable_write_boundary() {{\n                finish!(BlockExit::ExecutableWrite {{ source_bank: expected_bank, resume: ExecutionKey::new(expected_bank, GuestPc::new({next:#010X})) }});\n            }}\n"
        );
        if may_continue_locally {
            inline.push_str(&format!(
                "            if executed >= budget.get() {{\n                finish!(BlockExit::Checkpoint(ExecutionKey::new(expected_bank, GuestPc::new({next:#010X}))));\n            }}\n"
            ));
        }
        assert!(
            emitted.contains(&compact),
            "missing compact post-step at {next:#010X}"
        );
        removed_inline_bytes += inline.len() - compact.len();
    }
    let inline_equivalent_bytes = emitted.len() + removed_inline_bytes;
    eprintln!(
        "dense_source_compaction current_bytes={} inline_equivalent_bytes={} saved_bytes={} saved_percent={:.1}",
        emitted.len(),
        inline_equivalent_bytes,
        removed_inline_bytes,
        100.0 * removed_inline_bytes as f64 / inline_equivalent_bytes as f64,
    );
    assert!(
        emitted.len() * 4 <= inline_equivalent_bytes * 3,
        "shared post-step must remove at least 25% of this ordinary-instruction source"
    );
}

#[test]
fn dense_live_word_verification_is_table_backed_per_subrunner() {
    let words = vec![0u32; 512];
    let emitted = emit_dense_bank_shard_runner_function(&DenseBankShardInput {
        name: "run_dense_table_backed_verification",
        bank: BankId::new(0xD00C),
        image_vram_start: BASE,
        image_vram_end: BASE + 2048,
        artifact_vram_start: BASE,
        artifact_vram_end: BASE + 2048,
        shard_vram_start: BASE,
        words: &words,
        delay_lookahead: None,
        verify_live_words: true,
    })
    .unwrap();

    assert_eq!(
        emitted
            .matches("fn64_recomp_rs::verify_precompiled_instruction_word(")
            .count(),
        1,
        "one shared verifier must replace per-arm verification calls"
    );
    assert!(emitted.contains("const EXPECTED_WORDS: &[u32]"));
    assert!(
        emitted.contains("let expected_word = EXPECTED_WORDS[((pc - 0x80001000) / 4) as usize];")
    );
    assert_eq!(
        emitted
            .matches("verify_live_word!(expected_bank, mem, pc, expected_word, pc);")
            .count(),
        1
    );
    assert!(
        emitted.len() < 300_000,
        "a 512-word ordinary subrunner must remain below the measured compact-source ceiling: {} bytes",
        emitted.len()
    );
}

#[test]
fn dense_memory_fault_lowering_stays_shared_and_cold() {
    let words = vec![0x8c82_0000u32; 512]; // lw $v0,0($a0)
    let emitted = emit_dense_bank_shard_runner_function(&DenseBankShardInput {
        name: "run_dense_shared_memory_faults",
        bank: BankId::new(0xD00D),
        image_vram_start: BASE,
        image_vram_end: BASE + 2048,
        artifact_vram_start: BASE,
        artifact_vram_end: BASE + 2048,
        shard_vram_start: BASE,
        words: &words,
        delay_lookahead: None,
        verify_live_words: true,
    })
    .unwrap();

    assert_eq!(
        emitted.matches("finish!(address_error(").count(),
        words.len()
    );
    assert_eq!(
        emitted.matches("finish_data_access_error(").count(),
        words.len()
    );
    assert!(!emitted.contains("CpuFaultKind::Exception {"));
    assert!(!emitted.contains("let __architectural"));
    assert!(!emitted.contains(".is_architectural_exception()"));
    assert!(!emitted.contains(".into_cpu_fault_kind("));
    eprintln!("dense_shared_memory_fault_source_bytes={}", emitted.len());
    assert!(
        emitted.len() < 600_000,
        "a 512-word load runner exceeded the shared-fault source ceiling: {} bytes",
        emitted.len()
    );
}

#[test]
fn dense_direct_entry_on_control_shaped_delay_remains_control() {
    let bank = BankId::new(0xD002);
    let target = BASE + 0x18;
    let jump = 0x0800_0000 | ((target & 0x0fff_ffff) >> 2);
    let words = [0x1000_0002, jump, 0x2405_0009];
    let emitted = emit_dense_bank_shard_runner_function(&DenseBankShardInput {
        name: "run_dense_control_delay_entry",
        bank,
        image_vram_start: BASE,
        image_vram_end: BASE + 0x20,
        artifact_vram_start: BASE,
        artifact_vram_end: BASE + 0x20,
        shard_vram_start: BASE,
        words: &words,
        delay_lookahead: None,
        verify_live_words: true,
    })
    .unwrap();
    compile_and_run(
        &emitted,
        &format!(
            r#"
let mut storage = vec![0u8; 0x2000];
let mut mem = Rdram::new(&mut storage);
mem.store_w(0xffff_ffff_0000_0000 | {BASE:#010x}, 0x1000_0002);
mem.store_w(0xffff_ffff_0000_0000 | {:#010x}, {jump:#010x});
mem.store_w(0xffff_ffff_0000_0000 | {:#010x}, 0x2405_0009);
let mut ctx = RecompContext::new();
let run = run_dense_control_delay_entry(
    ExecutionKey::new(BankId::new({}), GuestPc::new({:#010x})),
    InstructionBudget::new(8).unwrap(),
    &mut ctx,
    &mut mem,
);
assert_eq!(run.instructions, 2);
assert_eq!(ctx.r(5), 9);
assert_eq!(run.exit, BlockExit::Transfer(ExecutionKey::new(
    BankId::new({}), GuestPc::new({target:#010x}),
)));
"#,
            BASE + 4,
            BASE + 8,
            bank.get(),
            BASE + 4,
            bank.get(),
        ),
    );
}

#[test]
fn dense_cross_artifact_fallthrough_uses_active_generation_resolver() {
    let bank = BankId::new(0xD003);
    let emitted = emit_dense_bank_shard_runner_function(&DenseBankShardInput {
        name: "run_dense_fallthrough",
        bank,
        image_vram_start: BASE,
        image_vram_end: BASE + 8,
        artifact_vram_start: BASE,
        artifact_vram_end: BASE + 4,
        shard_vram_start: BASE,
        words: &[0x2402_0001],
        delay_lookahead: None,
        verify_live_words: true,
    })
    .unwrap();
    compile_and_run(
        &emitted,
        &format!(
            r#"
let mut storage = vec![0u8; 0x2000];
let mut mem = Rdram::new(&mut storage);
mem.store_w(0xffff_ffff_0000_0000 | {BASE:#010x}, 0x2402_0001);
let mut ctx = RecompContext::new();
let run = run_dense_fallthrough(
    ExecutionKey::new(BankId::new({}), GuestPc::new({BASE:#010x})),
    InstructionBudget::new(8).unwrap(),
    &mut ctx,
    &mut mem,
);
assert_eq!(run.instructions, 1);
assert_eq!(ctx.r(2), 1);
assert_eq!(run.exit, BlockExit::ResolveTransfer {{
    source_bank: BankId::new({}),
    target_pc: GuestPc::new({:#010x}),
}});
"#,
            bank.get(),
            bank.get(),
            BASE + 4,
        ),
    );
}

#[test]
fn dense_fetch_identity_ignores_neighbor_data_and_rejects_changed_instruction() {
    let bank = BankId::new(0xD004);
    let emitted = emit_dense_bank_shard_runner_function(&DenseBankShardInput {
        name: "run_dense_exact_fetch",
        bank,
        image_vram_start: BASE,
        image_vram_end: BASE + 4,
        artifact_vram_start: BASE,
        artifact_vram_end: BASE + 4,
        shard_vram_start: BASE,
        words: &[0x2402_0001],
        delay_lookahead: None,
        verify_live_words: true,
    })
    .unwrap();
    compile_and_run(
        &emitted,
        &format!(
            r#"
let mut storage = vec![0u8; 0x2000];
let mut mem = Rdram::new(&mut storage);
mem.store_w(0xffff_ffff_0000_0000 | {:#010x}, 0xdead_beef);
mem.store_w(0xffff_ffff_0000_0000 | {BASE:#010x}, 0x2402_0001);
mem.store_w(0xffff_ffff_0000_0000 | {:#010x}, 0xcafe_babe);
let mut ctx = RecompContext::new();
let entry = ExecutionKey::new(BankId::new({}), GuestPc::new({BASE:#010x}));
let run = run_dense_exact_fetch(entry, InstructionBudget::new(2).unwrap(), &mut ctx, &mut mem);
assert_eq!(run.instructions, 1);
assert_eq!(ctx.r(2), 1);

mem.store_w(0xffff_ffff_0000_0000 | {BASE:#010x}, 0x2402_0002);
let run = run_dense_exact_fetch(entry, InstructionBudget::new(2).unwrap(), &mut ctx, &mut mem);
assert_eq!(run.instructions, 0);
match run.exit {{
    BlockExit::ImageChanged {{ at, miss }} => {{
        assert_eq!(at, entry);
        assert_eq!(miss.va_start, GuestPc::new({BASE:#010x}));
        assert_eq!(miss.byte_len, 4);
    }}
    other => panic!("changed instruction did not fail closed: {{other:?}}"),
}}
"#,
            BASE - 4,
            BASE + 4,
            bank.get(),
        ),
    );
}

#[test]
fn dense_branch_likely_verifies_delay_only_when_taken() {
    let bank = BankId::new(0xD005);
    let emitted = emit_dense_bank_shard_runner_function(&DenseBankShardInput {
        name: "run_dense_likely_fetch",
        bank,
        image_vram_start: BASE,
        image_vram_end: BASE + 12,
        artifact_vram_start: BASE,
        artifact_vram_end: BASE + 12,
        shard_vram_start: BASE,
        // beql zero,v0,+1; addiu a0,zero,7; nop
        words: &[0x5002_0001, 0x2404_0007, 0],
        delay_lookahead: None,
        verify_live_words: true,
    })
    .unwrap();
    compile_and_run(
        &emitted,
        &format!(
            r#"
let mut storage = vec![0u8; 0x2000];
let mut mem = Rdram::new(&mut storage);
mem.store_w(0xffff_ffff_0000_0000 | {BASE:#010x}, 0x5002_0001);
mem.store_w(0xffff_ffff_0000_0000 | {:#010x}, 0x2404_0008);
mem.store_w(0xffff_ffff_0000_0000 | {:#010x}, 0);
let entry = ExecutionKey::new(BankId::new({}), GuestPc::new({BASE:#010x}));
let mut ctx = RecompContext::new();
ctx.set_r32(2, 1);
let run = run_dense_likely_fetch(entry, InstructionBudget::new(2).unwrap(), &mut ctx, &mut mem);
assert_eq!(run.instructions, 2);
assert_eq!(ctx.r(4), 0);
assert_eq!(run.exit, BlockExit::Transfer(ExecutionKey::new(
    BankId::new({}), GuestPc::new({:#010x}),
)));

ctx.set_r32(2, 0);
let run = run_dense_likely_fetch(entry, InstructionBudget::new(2).unwrap(), &mut ctx, &mut mem);
assert_eq!(run.instructions, 0);
assert!(matches!(run.exit, BlockExit::ImageChanged {{ at, miss }}
    if at == entry && miss.va_start == GuestPc::new({:#010x})));
"#,
            BASE + 4,
            BASE + 8,
            bank.get(),
            bank.get(),
            BASE + 8,
            BASE + 4,
        ),
    );
}

#[test]
fn dense_final_control_without_delay_is_typed_error() {
    let error = emit_dense_bank_shard_runner_function(&DenseBankShardInput {
        name: "run_dense_missing_delay",
        bank: BankId::new(0xD006),
        image_vram_start: BASE,
        image_vram_end: BASE + 4,
        artifact_vram_start: BASE,
        artifact_vram_end: BASE + 4,
        shard_vram_start: BASE,
        words: &[0x03e0_0008],
        delay_lookahead: None,
        verify_live_words: true,
    })
    .unwrap_err();
    assert_eq!(
        error,
        DenseEmitError::MissingArchitecturalDelayWord { pc: BASE }
    );
}

/// A target on an earlier control transfer's delay slot executes that slot as
/// an ordinary instruction without separating the architectural pair.
#[test]
fn branch_into_delay_slot_keeps_control_and_delay_as_one_unit() {
    const BASE: u32 = 0x8000_2000;
    let block_words = [
        0x1000_0002, // beq $zero,$zero,+2 -> 0x8000_200c
        0x2404_0007, // addiu $a0,$zero,7 (delay slot and entry target)
        0x03E0_0008, // jr $ra
        0x0000_0000, // nop
    ];
    let blocks = [BankBlockInput {
        vram: BASE,
        words: &block_words,
    }];
    let emitted = emit_sparse_bank_runner(&SparseBankInput {
        name: "run_hazard_bank",
        bank: BankId::new(0xB1),
        blocks: &blocks,
    });
    assert!(emitted.contains("0x80002004 => {"), "{emitted}");

    let stdout = compile_and_run(
        &emitted,
        r#"    let mut storage = vec![0u8; 64];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    let run = run_hazard_bank(
        ExecutionKey::new(BankId::new(0xB1), GuestPc::new(0x8000_2004)),
        InstructionBudget::new(64).unwrap(),
        &mut ctx,
        &mut mem,
    );
    assert_eq!(ctx.r(4), 7, "delay slot ran as an ordinary instruction");
    println!("hazard entry ran addiu: a0={}", ctx.r(4));
    let _ = run;
"#,
    );
    assert!(stdout.contains("hazard entry ran addiu: a0=7"), "{stdout}");
}

/// Control-shaped bytes admitted as a delay slot remain a loud runtime trap;
/// they never become a nested transfer requiring another delay slot.
#[test]
fn control_shaped_delay_slot_emits_a_trap_and_compiles() {
    const BASE: u32 = 0x8000_3000;
    let block_words = [
        0x03E0_0008, // jr $ra
        0x0226_8008, // jr $s1, interpreted only as the delay-slot payload
    ];
    let blocks = [BankBlockInput {
        vram: BASE,
        words: &block_words,
    }];
    let emitted = emit_sparse_bank_runner(&SparseBankInput {
        name: "run_trap_bank",
        bank: BankId::new(0xB2),
        blocks: &blocks,
    });
    assert!(
        emitted.contains("has no admitted delay slot")
            || emitted.contains("architecturally UNPREDICTABLE"),
        "control-shaped delay slot must emit a trap:\n{emitted}"
    );

    let stdout = compile_and_run(
        &emitted,
        r#"    let mut storage = vec![0u8; 64];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    let run = run_trap_bank(
        ExecutionKey::new(BankId::new(0xDEAD), GuestPc::new(0x8000_3000)),
        InstructionBudget::new(64).unwrap(),
        &mut ctx,
        &mut mem,
    );
    assert!(matches!(run.exit, BlockExit::Fault(CpuFault { kind: CpuFaultKind::UnknownBank, .. })));
    println!("trap bank compiled and ran: exit={:?}", run.exit);
"#,
    );
    assert!(stdout.contains("trap bank compiled and ran"), "{stdout}");
}
