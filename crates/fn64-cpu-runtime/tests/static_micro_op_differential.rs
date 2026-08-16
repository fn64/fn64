//! Dense verified-shard differential for the deliberately partial,
//! non-production static-micro-op executor.

use std::path::PathBuf;
use std::process::Command;

use fn64_cpu_runtime::{
    AdmittedStaticMicroOpProgramV1, BankId, BlockExit, CpuFault, CpuFaultKind, ExecutionKey,
    GuestPc, InstructionBudget, Rdram, RecompContext,
};
use fn64_recomp_rs_codegen::{
    emit_dense_bank_shard_runner_function, pack_static_micro_ops_v1, pack_static_micro_ops_v2,
    DenseBankShardInput, StaticMicroOpSpanInput, StaticMicroOpSpanInputV2,
};

mod support;
use support::dev_interpreter_rlib;

const BASE: u32 = 0x8000_1000;

struct Case {
    name: &'static str,
    words: &'static [u32],
    actual_words: &'static [u32],
    entry_offset: u32,
    budget: u32,
    init_regs: &'static [(u8, u64)],
    expect_image_changed: bool,
}

#[test]
fn experimental_predecoded_slice_matches_dense_verified_shards() {
    let cases = [
        Case {
            name: "initial_branch_budget_one",
            words: &[0x1000_0001, 0x2404_0007, 0x2405_0009],
            actual_words: &[0x1000_0001, 0x2404_0007, 0x2405_0009],
            entry_offset: 0,
            budget: 1,
            init_regs: &[],
            expect_image_changed: false,
        },
        Case {
            name: "interior_checkpoint",
            words: &[0x2402_0001, 0x2442_0002, 0x2403_0004, 0x2404_0008],
            actual_words: &[0x2402_0001, 0x2442_0002, 0x2403_0004, 0x2404_0008],
            entry_offset: 4,
            budget: 2,
            init_regs: &[],
            expect_image_changed: false,
        },
        Case {
            name: "beq_taken",
            words: &[0x1000_0001, 0x2404_0007, 0x2405_0009],
            actual_words: &[0x1000_0001, 0x2404_0007, 0x2405_0009],
            entry_offset: 0,
            budget: 8,
            init_regs: &[],
            expect_image_changed: false,
        },
        Case {
            name: "beq_not_taken",
            words: &[0x1043_0001, 0x2404_0007, 0x2405_0009],
            actual_words: &[0x1043_0001, 0x2404_0007, 0x2405_0009],
            entry_offset: 0,
            budget: 8,
            init_regs: &[(2, 1), (3, 2)],
            expect_image_changed: false,
        },
        Case {
            name: "beql_annuls_changed_delay",
            words: &[0x5040_0001, 0x2404_0007, 0x2405_0009],
            actual_words: &[0x5040_0001, 0x2404_00ff, 0x2405_0009],
            entry_offset: 0,
            budget: 8,
            init_regs: &[(2, 1)],
            expect_image_changed: false,
        },
        Case {
            name: "primary_live_mismatch",
            words: &[0x2402_0001, 0x2403_0002],
            actual_words: &[0x2402_0009, 0x2403_0002],
            entry_offset: 0,
            budget: 8,
            init_regs: &[],
            expect_image_changed: true,
        },
        Case {
            name: "delay_live_mismatch",
            words: &[0x1000_0001, 0x2404_0007, 0x2405_0009],
            actual_words: &[0x1000_0001, 0x2404_00ff, 0x2405_0009],
            entry_offset: 0,
            budget: 8,
            init_regs: &[],
            expect_image_changed: true,
        },
        Case {
            name: "reserved_straight",
            words: &[0x4c00_0000],
            actual_words: &[0x4c00_0000],
            entry_offset: 0,
            budget: 8,
            init_regs: &[],
            expect_image_changed: false,
        },
        Case {
            name: "reserved_delay",
            words: &[0x1000_0001, 0x4c00_0000, 0x2405_0009],
            actual_words: &[0x1000_0001, 0x4c00_0000, 0x2405_0009],
            entry_offset: 0,
            budget: 8,
            init_regs: &[],
            expect_image_changed: false,
        },
        Case {
            name: "control_shaped_word_direct_entry",
            words: &[0x1000_0001, 0x1000_0000, 0x0000_0000],
            actual_words: &[0x1000_0001, 0x1000_0000, 0x0000_0000],
            entry_offset: 4,
            budget: 8,
            init_regs: &[],
            expect_image_changed: false,
        },
        Case {
            name: "shared_integer_semantics",
            words: &[0x3443_ff00, 0x0003_2880, 0x00a2_302d],
            actual_words: &[0x3443_ff00, 0x0003_2880, 0x00a2_302d],
            entry_offset: 0,
            budget: 8,
            init_regs: &[(2, 1)],
            expect_image_changed: false,
        },
        Case {
            name: "shared_memory_round_trip",
            words: &[0xac83_0100, 0x8c82_0100],
            actual_words: &[0xac83_0100, 0x8c82_0100],
            entry_offset: 0,
            budget: 8,
            init_regs: &[(3, 0x1122_3344), (4, 0xffff_ffff_8000_0000)],
            expect_image_changed: false,
        },
        Case {
            name: "direct_mmio_hook_round_trip",
            words: &[0xac83_0018, 0x8c82_0018],
            actual_words: &[0xac83_0018, 0x8c82_0018],
            entry_offset: 0,
            budget: 8,
            init_regs: &[(3, 0x1122_3344), (4, 0xffff_ffff_a480_0000)],
            expect_image_changed: false,
        },
        Case {
            name: "mapped_tlb_alias_round_trip",
            words: &[0xac83_0100, 0x8c82_0100],
            actual_words: &[0xac83_0100, 0x8c82_0100],
            entry_offset: 0,
            budget: 8,
            init_regs: &[(3, 0x99aa_bbcc), (4, 0x0040_0000)],
            expect_image_changed: false,
        },
        Case {
            name: "shared_executable_write",
            words: &[0xac83_0100, 0x2402_0001],
            actual_words: &[0xac83_0100, 0x2402_0001],
            entry_offset: 0,
            budget: 8,
            init_regs: &[(3, 0x5566_7788), (4, 0xffff_ffff_8000_0000)],
            expect_image_changed: false,
        },
        Case {
            name: "shared_address_error_load",
            words: &[0x8c82_0000],
            actual_words: &[0x8c82_0000],
            entry_offset: 0,
            budget: 8,
            init_regs: &[(4, 0xffff_ffff_8000_0001)],
            expect_image_changed: false,
        },
        Case {
            name: "shared_cop0_random",
            words: &[0x4002_0800],
            actual_words: &[0x4002_0800],
            entry_offset: 0,
            budget: 8,
            init_regs: &[],
            expect_image_changed: false,
        },
        Case {
            name: "multi_straight_cop0_random_count",
            words: &[0x4002_0800, 0x4003_4800, 0x4004_0800, 0x4005_4800],
            actual_words: &[0x4002_0800, 0x4003_4800, 0x4004_0800, 0x4005_4800],
            entry_offset: 0,
            budget: 8,
            init_regs: &[],
            expect_image_changed: false,
        },
        Case {
            name: "eret_step_exit",
            words: &[0x4200_0018],
            actual_words: &[0x4200_0018],
            entry_offset: 0,
            budget: 8,
            init_regs: &[],
            expect_image_changed: false,
        },
        Case {
            name: "straight_fault_after_prior_retirement",
            words: &[0x2402_0001, 0x4c00_0000],
            actual_words: &[0x2402_0001, 0x4c00_0000],
            entry_offset: 0,
            budget: 8,
            init_regs: &[],
            expect_image_changed: false,
        },
        Case {
            name: "shared_cop1_unusable",
            words: &[0x4402_0000],
            actual_words: &[0x4402_0000],
            entry_offset: 0,
            budget: 8,
            init_regs: &[],
            expect_image_changed: false,
        },
        Case {
            name: "shared_cop1_add_s",
            words: &[0x4602_0100],
            actual_words: &[0x4602_0100],
            entry_offset: 0,
            budget: 8,
            init_regs: &[],
            expect_image_changed: false,
        },
    ];

    let mut emitted = String::new();
    let mut checks = String::new();
    for (index, case) in cases.iter().enumerate() {
        let bank = BankId::new(0x5a00 + index as u64);
        let end = BASE + case.words.len() as u32 * 4;
        let runner_name = format!("run_static_micro_op_dense_{index}");
        emitted.push_str(
            &emit_dense_bank_shard_runner_function(&DenseBankShardInput {
                name: &runner_name,
                bank,
                image_vram_start: BASE,
                image_vram_end: end,
                artifact_vram_start: BASE,
                artifact_vram_end: end,
                shard_vram_start: BASE,
                words: case.words,
                delay_lookahead: None,
                verify_live_words: true,
            })
            .expect("emit dense differential shard"),
        );
        let packed = pack_static_micro_ops_v1(&[StaticMicroOpSpanInput {
            bank,
            vram: BASE,
            words: case.words,
        }])
        .expect("pack differential program");
        let packed_bytes = packed
            .bytes()
            .iter()
            .map(|byte| format!("{byte}u8"))
            .collect::<Vec<_>>()
            .join(",");
        let actual_words = case
            .actual_words
            .iter()
            .map(|word| format!("{word:#010x}u32"))
            .collect::<Vec<_>>()
            .join(",");
        let init_regs = case
            .init_regs
            .iter()
            .map(|(register, value)| format!("({register}u8, {value}u64)"))
            .collect::<Vec<_>>()
            .join(",");
        checks.push_str(&format!(
            "check({name:?}, BankId::new({bank_id}), {entry:#010x}, {budget}, &[{actual_words}], &[{packed_bytes}], &[{init_regs}], {runner_name}, {expect_image_changed});\n",
            name = case.name,
            bank_id = bank.get(),
            entry = BASE + case.entry_offset,
            budget = case.budget,
            expect_image_changed = case.expect_image_changed,
        ));
    }

    let v2_bank = BankId::new(0x5afe);
    let v2_runner = "run_static_micro_op_v2_lookahead";
    emitted.push_str(
        &emit_dense_bank_shard_runner_function(&DenseBankShardInput {
            name: v2_runner,
            bank: v2_bank,
            image_vram_start: BASE,
            image_vram_end: BASE + 8,
            artifact_vram_start: BASE,
            artifact_vram_end: BASE + 4,
            shard_vram_start: BASE,
            words: &[0x1000_0000],
            delay_lookahead: Some(0),
            verify_live_words: true,
        })
        .unwrap(),
    );
    let v2_pack = pack_static_micro_ops_v2(&[StaticMicroOpSpanInputV2 {
        bank: v2_bank,
        vram: BASE,
        words: &[0x1000_0000],
        delay_lookahead: Some(0),
    }])
    .unwrap();
    let v2_bytes = v2_pack
        .bytes()
        .iter()
        .map(|byte| format!("{byte}u8"))
        .collect::<Vec<_>>()
        .join(",");
    checks.push_str(&format!(
        "check_v2_lookahead(BankId::new({}), &[{}], {});\n",
        v2_bank.get(),
        v2_bytes,
        v2_runner,
    ));

    compile_and_run(&emitted, &checks, cases.len() + 3);
}

#[test]
fn control_shaped_delay_fails_only_when_that_pair_executes() {
    let bank = BankId::new(0x5aff);
    let words = [0x1000_0001, 0x1000_0000, 0x0000_0000];
    let packed = pack_static_micro_ops_v1(&[StaticMicroOpSpanInput {
        bank,
        vram: BASE,
        words: &words,
    }])
    .expect("control-shaped delay is valid pack data");
    let program = AdmittedStaticMicroOpProgramV1::from_bytes(packed.bytes()).unwrap();

    let mut storage = vec![0u8; 0x2000];
    let mut ctx = RecompContext::new();
    let run = {
        let mut mem = Rdram::new(&mut storage);
        for (index, word) in words.iter().copied().enumerate() {
            mem.store_w(
                0xffff_ffff_8000_1000 + u64::try_from(index).unwrap() * 4,
                word,
            );
        }
        program.run(
            ExecutionKey::new(bank, GuestPc::new(BASE)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
    };
    assert_eq!(run.instructions, 2);
    assert!(matches!(
        run.exit,
        BlockExit::Fault(CpuFault {
            at,
            kind: CpuFaultKind::UnsupportedInstruction { word: 0x1000_0000 },
        }) if at == ExecutionKey::new(bank, GuestPc::new(BASE + 4))
    ));
    assert_eq!(
        ctx.read_cop0(1),
        30,
        "only the owning branch advances Random"
    );
}

fn compile_and_run(emitted: &str, checks: &str, case_count: usize) {
    let source = format!(
        r#"#![allow(unused_imports, unused_variables, unused_mut)]
use fn64_cpu_runtime::{{
    AdmittedStaticMicroOpProgramV1, AdmittedStaticMicroOpProgramV2, BankId, BlockExit, BlockRun, CpuException, CpuFault,
    CpuFaultKind, ExecutionKey, GuestPc, GuestWriteBoundary, GuestWriteEvent, InstructionBudget,
    Rdram, RecompContext, RecompContextEvidenceSnapshotV1, TlbEntryRaw,
    set_guest_write_boundary_observer, set_mmio_hooks,
}};

{emitted}

type DenseRunner = fn(ExecutionKey, InstructionBudget, &mut RecompContext, &mut Rdram) -> BlockRun;

#[derive(Debug, PartialEq, Eq)]
struct State {{
    cpu: RecompContextEvidenceSnapshotV1,
    rdram: Vec<u8>,
    mmio: Vec<(u64, Option<u32>)>,
}}

fn snapshot(ctx: &RecompContext, mem: &[u8]) -> State {{
    State {{
        cpu: ctx.evidence_snapshot_v1(),
        rdram: mem.to_vec(),
        mmio: MMIO_EVENTS.with(|events| events.borrow().clone()),
    }}
}}

thread_local! {{
    static MMIO_EVENTS: std::cell::RefCell<Vec<(u64, Option<u32>)>> = const {{ std::cell::RefCell::new(Vec::new()) }};
}}

fn differential_mmio_read(address: u64) -> Option<u32> {{
    if address == 0xffff_ffff_a480_0018 {{
        MMIO_EVENTS.with(|events| events.borrow_mut().push((address, None)));
        Some(0x89ab_cdef)
    }} else {{
        None
    }}
}}

fn differential_mmio_write(address: u64, value: u32) -> bool {{
    if address == 0xffff_ffff_a480_0018 {{
        MMIO_EVENTS.with(|events| events.borrow_mut().push((address, Some(value))));
        true
    }} else {{
        false
    }}
}}

fn make_state(actual_words: &[u32], init_regs: &[(u8, u64)]) -> (RecompContext, Vec<u8>) {{
    let mut ctx = RecompContext::new();
    for &(register, value) in init_regs {{
        ctx.set_r(register, value);
    }}
    let mut storage = vec![0u8; 0x2000];
    {{
        let mut mem = Rdram::new(&mut storage);
        for (index, word) in actual_words.iter().copied().enumerate() {{
            mem.store_w(0xffff_ffff_8000_1000 + index as u64 * 4, word);
        }}
    }}
    (ctx, storage)
}}

fn configure_case(name: &str, ctx: &mut RecompContext) {{
    if name == "shared_cop1_add_s" {{
        ctx.cop0_status |= 1 << 29;
        ctx.set_f_bits(0, 1.5f32.to_bits());
        ctx.set_f_bits(2, 2.25f32.to_bits());
    }}
    if name == "mapped_tlb_alias_round_trip" {{
        ctx.tlb_entries[0] = TlbEntryRaw {{
            page_mask: 0,
            entry_hi: 0x0040_0000,
            entry_lo0: 0b111,
            entry_lo1: 0b111,
        }};
    }}
    if name == "eret_step_exit" {{
        ctx.cop0_status = 1 << 1;
        ctx.cop0_epc = 0x8000_9000;
        ctx.set_ll_reservation(0x8000_0040, 4);
    }}
}}

fn prepare_external_case(name: &str) -> (Option<fn(u64) -> Option<u32>>, Option<fn(u64, u32) -> bool>) {{
    MMIO_EVENTS.with(|events| events.borrow_mut().clear());
    if name == "direct_mmio_hook_round_trip" {{
        set_mmio_hooks(Some(differential_mmio_read), Some(differential_mmio_write))
    }} else {{
        set_mmio_hooks(None, None)
    }}
}}

fn assert_case_outcome(name: &str, bank: BankId, run: BlockRun, state: &State) {{
    match name {{
        "initial_branch_budget_one" => {{
            assert_eq!(run.instructions, 0);
            assert_eq!(
                run.exit,
                BlockExit::Checkpoint(ExecutionKey::new(bank, GuestPc::new(0x8000_1000)))
            );
            assert_eq!(state.cpu.gprs[4], 0);
            assert_eq!(state.cpu.gprs[5], 0);
        }}
        "direct_mmio_hook_round_trip" => {{
            assert_eq!(
                state.mmio,
                vec![
                    (0xffff_ffff_a480_0018, Some(0x1122_3344)),
                    (0xffff_ffff_a480_0018, None),
                ],
                "MMIO SW/LW did not traverse both canonical hooks in program order"
            );
            assert_eq!(state.cpu.gprs[2], 0xffff_ffff_89ab_cdef);
        }}
        "mapped_tlb_alias_round_trip" => {{
            assert_eq!(
                &state.rdram[0x100..0x104],
                &0x99aa_bbccu32.to_le_bytes(),
                "mapped SW did not commit through the TLB alias"
            );
            assert_eq!(state.cpu.gprs[2], 0xffff_ffff_99aa_bbcc);
        }}
        "eret_step_exit" => {{
            assert_eq!(run.instructions, 1);
            assert_eq!(
                run.exit,
                BlockExit::ResolveTransfer {{
                    source_bank: bank,
                    target_pc: GuestPc::new(0x8000_9000),
                }}
            );
            assert_eq!(state.cpu.cop0_status & (1 << 1), 0);
            assert_eq!(state.cpu.ll_reservation, None);
        }}
        "straight_fault_after_prior_retirement" => {{
            assert_eq!(run.instructions, 2);
            assert!(matches!(
                run.exit,
                BlockExit::Fault(CpuFault {{
                    at,
                    kind: CpuFaultKind::Exception {{
                        exception: CpuException::ReservedInstruction,
                        branch_delay: false,
                        ..
                    }},
                }}) if at == ExecutionKey::new(bank, GuestPc::new(0x8000_1004))
            ), "unexpected post-retirement fault: {{run:?}}");
            assert_eq!(state.cpu.gprs[2], 1);
        }}
        "multi_straight_cop0_random_count" => {{
            assert_eq!(run.instructions, 4);
            assert_eq!(state.cpu.gprs[2], 31);
            assert_eq!(state.cpu.gprs[3], 0);
            assert_eq!(state.cpu.gprs[4], 29);
            assert_eq!(state.cpu.gprs[5], 1);
        }}
        _ => {{}}
    }}
}}

fn executable_boundary(event: GuestWriteEvent) -> GuestWriteBoundary {{
    let (start, len) = event.range();
    let end = start + len;
    if start < 0x104 && end > 0x100 {{
        GuestWriteBoundary::ExecutableChanged
    }} else {{
        GuestWriteBoundary::Continue
    }}
}}

#[allow(clippy::too_many_arguments)]
fn check(
    name: &str,
    bank: BankId,
    entry: u32,
    budget: u32,
    actual_words: &[u32],
    packed: &[u8],
    init_regs: &[(u8, u64)],
    dense: DenseRunner,
    expect_image_changed: bool,
) {{
    let key = ExecutionKey::new(bank, GuestPc::new(entry));
    let budget = InstructionBudget::new(budget).unwrap();
    let program = AdmittedStaticMicroOpProgramV1::from_bytes(packed).unwrap();

    let (mut dense_ctx, mut dense_storage) = make_state(actual_words, init_regs);
    configure_case(name, &mut dense_ctx);
    let previous_mmio = prepare_external_case(name);
    if name == "shared_executable_write" {{
        set_guest_write_boundary_observer(Some(executable_boundary));
    }}
    let dense_run = {{
        let mut mem = Rdram::new(&mut dense_storage);
        dense(key, budget, &mut dense_ctx, &mut mem)
    }};
    set_guest_write_boundary_observer(None);
    let dense_state = snapshot(&dense_ctx, &dense_storage);
    set_mmio_hooks(previous_mmio.0, previous_mmio.1);

    let (mut packed_ctx, mut packed_storage) = make_state(actual_words, init_regs);
    configure_case(name, &mut packed_ctx);
    let previous_mmio = prepare_external_case(name);
    if name == "shared_executable_write" {{
        set_guest_write_boundary_observer(Some(executable_boundary));
    }}
    let packed_run = {{
        let mut mem = Rdram::new(&mut packed_storage);
        program.run(key, budget, &mut packed_ctx, &mut mem)
    }};
    set_guest_write_boundary_observer(None);
    let packed_state = snapshot(&packed_ctx, &packed_storage);
    set_mmio_hooks(previous_mmio.0, previous_mmio.1);

    assert_eq!(dense_run, packed_run, "{{name}} BlockRun diverged");
    assert_eq!(dense_state, packed_state, "{{name}} state diverged");
    assert_eq!(
        matches!(packed_run.exit, BlockExit::ImageChanged {{ .. }}),
        expect_image_changed,
        "{{name}} ImageChanged classification diverged from its fixture"
    );
    assert_case_outcome(name, bank, packed_run, &packed_state);
}}

fn check_v2_lookahead(bank: BankId, packed: &[u8], dense: DenseRunner) {{
    let program = AdmittedStaticMicroOpProgramV2::from_bytes(packed).unwrap();
    assert_eq!(program.instruction_count(), 1, "lookahead is not owned");
    let budget = InstructionBudget::new(8).unwrap();

    for (name, lookahead) in [("matching", 0u32), ("changed", 0x2404_0007u32)] {{
        let actual = [0x1000_0000, lookahead];
        let (mut dense_ctx, mut dense_storage) = make_state(&actual, &[]);
        let dense_run = {{
            let mut mem = Rdram::new(&mut dense_storage);
            dense(ExecutionKey::new(bank, GuestPc::new(0x8000_1000)), budget, &mut dense_ctx, &mut mem)
        }};
        let dense_state = snapshot(&dense_ctx, &dense_storage);
        let (mut packed_ctx, mut packed_storage) = make_state(&actual, &[]);
        let packed_run = {{
            let mut mem = Rdram::new(&mut packed_storage);
            program.run(ExecutionKey::new(bank, GuestPc::new(0x8000_1000)), budget, &mut packed_ctx, &mut mem)
        }};
        assert_eq!(dense_run, packed_run, "v2 {{name}} lookahead exit diverged");
        assert_eq!(dense_state, snapshot(&packed_ctx, &packed_storage), "v2 {{name}} state diverged");
        assert_eq!(matches!(packed_run.exit, BlockExit::ImageChanged {{ .. }}), name == "changed");
    }}

    let actual = [0x1000_0000, 0];
    let direct = ExecutionKey::new(bank, GuestPc::new(0x8000_1004));
    let (mut dense_ctx, mut dense_storage) = make_state(&actual, &[]);
    let dense_run = {{
        let mut mem = Rdram::new(&mut dense_storage);
        dense(direct, budget, &mut dense_ctx, &mut mem)
    }};
    let (mut packed_ctx, mut packed_storage) = make_state(&actual, &[]);
    let packed_run = {{
        let mut mem = Rdram::new(&mut packed_storage);
        program.run(direct, budget, &mut packed_ctx, &mut mem)
    }};
    assert_eq!(dense_run, packed_run, "delay-only lookahead became a direct entry");
    assert!(matches!(packed_run.exit, BlockExit::Fault(CpuFault {{ kind: CpuFaultKind::UnmappedPc {{ .. }}, .. }})));
}}

fn main() {{
{checks}
    println!("static-micro-op differential ok: {case_count} cases");
}}
"#
    );

    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let key = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let process = std::process::id();
    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let source_path = out_dir.join(format!("fn64_static_micro_op_{process}_{key}.rs"));
    let binary_path = out_dir.join(format!("fn64_static_micro_op_{process}_{key}"));
    std::fs::write(&source_path, source).expect("write isolated differential source");

    let deps = std::env::current_exe()
        .expect("current integration test")
        .parent()
        .expect("target deps")
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
        .expect("compile isolated differential");
    assert!(
        compile.status.success(),
        "static-micro-op differential did not compile:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&binary_path)
        .output()
        .expect("run isolated differential");
    assert!(
        run.status.success(),
        "static-micro-op differential failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(String::from_utf8_lossy(&run.stdout).contains(&format!(
        "static-micro-op differential ok: {case_count} cases"
    )));
}
