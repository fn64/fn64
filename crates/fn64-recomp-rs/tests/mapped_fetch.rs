//! Differential gate for canonical 32-bit mapped instruction fetch.
//!
//! The emitted AOT lane and `dynamic_mips` interpreter both execute one
//! fetch-validated unit at a time. The cases below make virtual control-flow
//! state differ from physical word identity, including a branch whose adjacent
//! delay-slot VA is backed by a nonadjacent physical page.

use fn64_recomp_rs::BankId;
use fn64_recomp_rs_codegen::{emit_bank_runner, BankInput};
use std::path::PathBuf;
use std::process::Command;

mod support;
use support::dev_interpreter_rlib;

#[test]
fn mapped_fetch_keeps_virtual_architecture_and_physical_generation_separate_across_lanes() {
    const ALIAS_WORD: u32 = 0x2442_0001; // addiu $v0,$v0,1
    const REMAP_WORD: u32 = 0x2442_0007; // addiu $v0,$v0,7
    const BRANCH_WORD: u32 = 0x1000_0001; // beq $zero,$zero,+1
    const DELAY_WORD: u32 = 0x2442_0005; // addiu $v0,$v0,5

    let bank_a = BankId::new(0xA001);
    let bank_b = BankId::new(0xA002);
    let bank_c = BankId::new(0xA003);
    let mut emitted = String::new();
    for (name, bank, vram, words) in [
        ("alias_first", bank_a, 0x0040_0000, vec![ALIAS_WORD]),
        ("alias_second", bank_a, 0x0080_0000, vec![ALIAS_WORD]),
        ("remapped", bank_b, 0x0040_0000, vec![REMAP_WORD]),
        ("direct_kseg0", bank_c, 0x8000_0040, vec![ALIAS_WORD]),
        ("direct_kseg1", bank_c, 0xa000_0040, vec![ALIAS_WORD]),
        (
            "cross_page_branch",
            bank_a,
            0x0040_0ffc,
            vec![BRANCH_WORD, DELAY_WORD],
        ),
    ] {
        emitted.push_str(&emit_bank_runner(&BankInput {
            name,
            bank,
            vram,
            words: &words,
        }));
        emitted.push('\n');
    }

    let source = format!(
        r#"#![allow(unused_imports)]
use fn64_recomp_rs::{{
    fetch_instruction, run_mapped_bank, BankId, BlockExit, BlockProgram, BlockRun, CodeBank,
    CpuException, CpuFault, CpuFaultKind, ExecutionKey, FetchedInstruction,
    GeneratedBankRunner, GuestPc, InstructionFetchSite, InstructionBudget, MappedAotBlock,
    PhysicalCodeBank, PhysicalCodeCatalog, ProgramError, Rdram, RecompContext, TlbEntryRaw,
}};

{emitted}

const ALIAS_WORD: u32 = {ALIAS_WORD:#010X};
const REMAP_WORD: u32 = {REMAP_WORD:#010X};
const BRANCH_WORD: u32 = {BRANCH_WORD:#010X};
const DELAY_WORD: u32 = {DELAY_WORD:#010X};

fn entry_lo(physical_page: u32, valid: bool) -> u32 {{
    ((physical_page >> 6) & 0x03ff_ffc0) | 1 | ((valid as u32) << 1) | (1 << 2)
}}

fn map_pair(ctx: &mut RecompContext, index: usize, virtual_pair: u32, even_pa: u32, odd_pa: u32) {{
    ctx.tlb_entries[index] = TlbEntryRaw {{
        page_mask: 0,
        entry_hi: u64::from(virtual_pair & 0xffff_e000),
        entry_lo0: entry_lo(even_pa, true),
        entry_lo1: entry_lo(odd_pa, true),
    }};
}}

fn build_catalog() -> PhysicalCodeCatalog {{
    let mut catalog = PhysicalCodeCatalog::new();
    catalog.register(PhysicalCodeBank::from_spans(
        BankId::new(0xA001),
        vec![
            fn64_recomp_rs::PhysicalCodeSpan::new(
                BankId::new(0xA001), 0x0010_0000, vec![ALIAS_WORD]
            ).unwrap(),
            fn64_recomp_rs::PhysicalCodeSpan::new(
                BankId::new(0xA001), 0x0010_0ffc, vec![BRANCH_WORD]
            ).unwrap(),
            fn64_recomp_rs::PhysicalCodeSpan::new(
                BankId::new(0xA001), 0x0030_0000, vec![DELAY_WORD]
            ).unwrap(),
        ],
    ).unwrap()).unwrap();
    catalog.register(PhysicalCodeBank::new(
        BankId::new(0xA002), 0x0020_0000, vec![REMAP_WORD]
    ).unwrap()).unwrap();
    catalog.register(PhysicalCodeBank::new(
        BankId::new(0xA003), 0x0000_0040, vec![ALIAS_WORD]
    ).unwrap()).unwrap();
    catalog
}}

fn state(ctx: &RecompContext) -> ([u64; 32], u32, u32, u32, u64, u32, u64) {{
    (
        ctx.gprs(), ctx.cop0_status, ctx.cop0_cause, ctx.cop0_epc,
        ctx.cop0_badvaddr, ctx.cop0_context, ctx.cop0_entry_hi,
    )
}}

fn compare_unit(
    catalog: &PhysicalCodeCatalog,
    template: &RecompContext,
    block: MappedAotBlock,
    bank: BankId,
    entry: GuestPc,
) {{
    let budget = InstructionBudget::new(2).unwrap();
    let mut interp_ctx = template.clone();
    let mut aot_ctx = template.clone();
    let mut interp_bytes = vec![0; 0x100];
    let mut aot_bytes = interp_bytes.clone();
    let mut interp_program = BlockProgram::new();
    interp_program.register_physical_code(catalog.bank(bank).unwrap().clone()).unwrap();
    let interp = interp_program.run(
        fn64_recomp_rs::ExecutionKey::new(bank, entry), budget,
        &mut interp_ctx, &mut Rdram::new(&mut interp_bytes),
    );
    let mut program = BlockProgram::new();
    program.register_physical_code(catalog.bank(bank).unwrap().clone()).unwrap();
    program.register_mapped_aot(block).unwrap();
    let aot = program.run(
        fn64_recomp_rs::ExecutionKey::new(bank, entry), budget,
        &mut aot_ctx, &mut Rdram::new(&mut aot_bytes),
    );
    assert_eq!(interp, aot);
    assert_eq!(state(&interp_ctx), state(&aot_ctx));
    assert_eq!(interp_bytes, aot_bytes);
}}

fn main() {{
    let catalog = build_catalog();
    let bank_a = BankId::new(0xA001);
    let bank_b = BankId::new(0xA002);
    let bank_c = BankId::new(0xA003);
    let mut ctx = RecompContext::new();

    // Two unrelated VAs select the exact same admitted physical word identity.
    map_pair(&mut ctx, 0, 0x0040_0000, 0x0010_0000, 0x0010_1000);
    map_pair(&mut ctx, 1, 0x0080_0000, 0x0010_0000, 0x0010_1000);
    let first_fetch = fetch_instruction(
        &catalog, &ctx, bank_a, InstructionFetchSite::primary(GuestPc::new(0x0040_0000))
    ).unwrap();
    let second_fetch = fetch_instruction(
        &catalog, &ctx, bank_a, InstructionFetchSite::primary(GuestPc::new(0x0080_0000))
    ).unwrap();
    assert_eq!(first_fetch.identity, second_fetch.identity);
    assert_ne!(first_fetch.virtual_pc, second_fetch.virtual_pc);

    let first = MappedAotBlock::new(
        &catalog, &ctx, bank_a, GuestPc::new(0x0040_0000), &[ALIAS_WORD],
        GeneratedBankRunner::new(bank_a, alias_first),
    ).unwrap();
    let second = MappedAotBlock::new(
        &catalog, &ctx, bank_a, GuestPc::new(0x0080_0000), &[ALIAS_WORD],
        GeneratedBankRunner::new(bank_a, alias_second),
    ).unwrap();
    compare_unit(&catalog, &ctx, first, bank_a, GuestPc::new(0x0040_0000));
    compare_unit(&catalog, &ctx, second, bank_a, GuestPc::new(0x0080_0000));

    let kseg0_fetch = fetch_instruction(
        &catalog, &ctx, bank_c, InstructionFetchSite::primary(GuestPc::new(0x8000_0040))
    ).unwrap();
    let kseg1_fetch = fetch_instruction(
        &catalog, &ctx, bank_c, InstructionFetchSite::primary(GuestPc::new(0xa000_0040))
    ).unwrap();
    assert_eq!(kseg0_fetch.identity, kseg1_fetch.identity);
    compare_unit(
        &catalog, &ctx,
        MappedAotBlock::new(
            &catalog, &ctx, bank_c, GuestPc::new(0x8000_0040), &[ALIAS_WORD],
            GeneratedBankRunner::new(bank_c, direct_kseg0),
        ).unwrap(),
        bank_c, GuestPc::new(0x8000_0040),
    );
    compare_unit(
        &catalog, &ctx,
        MappedAotBlock::new(
            &catalog, &ctx, bank_c, GuestPc::new(0xa000_0040), &[ALIAS_WORD],
            GeneratedBankRunner::new(bank_c, direct_kseg1),
        ).unwrap(),
        bank_c, GuestPc::new(0xa000_0040),
    );

    // Remapping the same VA selects a different word and immutable generation.
    let old_identity = first_fetch.identity;
    map_pair(&mut ctx, 0, 0x0040_0000, 0x0020_0000, 0x0020_1000);
    let remapped_fetch = fetch_instruction(
        &catalog, &ctx, bank_b, InstructionFetchSite::primary(GuestPc::new(0x0040_0000))
    ).unwrap();
    assert_ne!(old_identity, remapped_fetch.identity);
    assert_eq!(remapped_fetch.word, REMAP_WORD);
    let remapped_block = MappedAotBlock::new(
        &catalog, &ctx, bank_b, GuestPc::new(0x0040_0000), &[REMAP_WORD],
        GeneratedBankRunner::new(bank_b, remapped),
    ).unwrap();
    compare_unit(&catalog, &ctx, remapped_block, bank_b, GuestPc::new(0x0040_0000));

    // The adjacent slot VA crosses from the even to odd 4 KiB page, whose PFN
    // is deliberately nonadjacent. Both lanes execute the PA-selected slot.
    map_pair(&mut ctx, 0, 0x0040_0000, 0x0010_0000, 0x0030_0000);
    let branch = MappedAotBlock::new(
        &catalog, &ctx, bank_a, GuestPc::new(0x0040_0ffc), &[BRANCH_WORD, DELAY_WORD],
        GeneratedBankRunner::new(bank_a, cross_page_branch),
    ).unwrap();
    assert_eq!(branch.identities()[0].physical_address, 0x0010_0ffc);
    assert_eq!(branch.identities()[1].physical_address, 0x0030_0000);
    compare_unit(&catalog, &ctx, branch, bank_a, GuestPc::new(0x0040_0ffc));

    let invalid_branch = MappedAotBlock::new(
        &catalog, &ctx, bank_a, GuestPc::new(0x0040_0ffc), &[BRANCH_WORD, DELAY_WORD],
        GeneratedBankRunner::new(bank_a, cross_page_branch),
    ).unwrap();

    let mut invalid_delay = ctx.clone();
    invalid_delay.tlb_entries[0].entry_lo1 &= !(1 << 1);
    let mut interp_delay_ctx = invalid_delay.clone();
    let mut aot_delay_ctx = invalid_delay;
    let mut interp_delay_bytes = vec![0; 0x100];
    let mut aot_delay_bytes = interp_delay_bytes.clone();
    let budget = InstructionBudget::new(2).unwrap();
    let interp_delay = run_mapped_bank(
        &catalog, bank_a, GuestPc::new(0x0040_0ffc), budget,
        &mut interp_delay_ctx, &mut Rdram::new(&mut interp_delay_bytes),
    ).unwrap();
    let aot_delay = invalid_branch.run(
        &catalog, budget, &mut aot_delay_ctx, &mut Rdram::new(&mut aot_delay_bytes),
    );
    assert_eq!(interp_delay, aot_delay);
    let BlockExit::Fault(delay_fault) = interp_delay.exit else {{ panic!("expected slot fault") }};
    assert_eq!(delay_fault.enter_exception(&mut interp_delay_ctx), Some(GuestPc::new(0x8000_0180)));
    assert_eq!(delay_fault.enter_exception(&mut aot_delay_ctx), Some(GuestPc::new(0x8000_0180)));
    assert_eq!(state(&interp_delay_ctx), state(&aot_delay_ctx));
    assert_eq!(interp_delay_ctx.cop0_epc, 0x0040_0ffc);
    assert_eq!(interp_delay_ctx.cop0_badvaddr, 0x0040_1000);
    assert_eq!(interp_delay_ctx.cop0_context, 0x0000_2000);
    assert_eq!(interp_delay_ctx.cop0_entry_hi, 0x0040_0000);
    assert_ne!(interp_delay_ctx.cop0_cause & (1 << 31), 0);

    // A refill and an invalid fetch agree across lanes, then commit precise
    // CP0 state and first-level/common vector selection.
    let fault_block = MappedAotBlock::new(
        &catalog, &ctx, bank_a, GuestPc::new(0x0040_0000), &[ALIAS_WORD],
        GeneratedBankRunner::new(bank_a, alias_first),
    ).unwrap();
    let valid_template = ctx.clone();
    for invalid in [false, true] {{
        let mut fault_ctx = valid_template.clone();
        if invalid {{
            fault_ctx.tlb_entries[0].entry_lo0 &= !(1 << 1);
        }} else {{
            fault_ctx.tlb_entries[0] = TlbEntryRaw::default();
        }}
        let mut interp_ctx = fault_ctx.clone();
        let mut aot_ctx = fault_ctx;
        let mut interp_bytes = vec![0; 0x100];
        let mut aot_bytes = interp_bytes.clone();
        let budget = InstructionBudget::new(2).unwrap();
        let interp = run_mapped_bank(
            &catalog, bank_a, GuestPc::new(0x0040_0000), budget,
            &mut interp_ctx, &mut Rdram::new(&mut interp_bytes),
        ).unwrap();
        let aot = fault_block.run(
            &catalog, budget, &mut aot_ctx, &mut Rdram::new(&mut aot_bytes),
        );
        assert_eq!(interp, aot);
        let BlockExit::Fault(fault) = interp.exit else {{ panic!("expected fetch fault") }};
        assert_eq!(
            fault.enter_exception(&mut interp_ctx),
            Some(GuestPc::new(if invalid {{ 0x8000_0180 }} else {{ 0x8000_0000 }})),
        );
        assert_eq!(
            fault.enter_exception(&mut aot_ctx),
            Some(GuestPc::new(if invalid {{ 0x8000_0180 }} else {{ 0x8000_0000 }})),
        );
        assert_eq!(state(&interp_ctx), state(&aot_ctx));
        assert_eq!(interp_ctx.cop0_epc, 0x0040_0000);
        assert_eq!(interp_ctx.cop0_badvaddr, 0x0040_0000);
        assert_eq!(interp_ctx.cop0_context, 0x0000_2000);
        assert_eq!(interp_ctx.cop0_entry_hi, 0x0040_0000);
        assert_eq!(interp_ctx.cop0_cause & (1 << 31), 0);
        assert_eq!((interp_ctx.cop0_cause >> 2) & 0x1f, 2);
    }}
}}
"#
    );

    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let source_path = out_dir.join("fn64_mapped_fetch_gate.rs");
    let binary_path = out_dir.join("fn64_mapped_fetch_gate");
    std::fs::write(&source_path, source).expect("write mapped-fetch harness source");

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
        .expect("invoke rustc for mapped-fetch harness");
    assert!(
        compile.status.success(),
        "mapped-fetch harness did not compile:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&binary_path)
        .output()
        .expect("run mapped-fetch harness");
    assert!(
        run.status.success(),
        "mapped-fetch harness failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}
