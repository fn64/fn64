//! Differential correctness gate for the `dynamic_mips` interpreter lane.
//!
//! The interpreter ([`fn64_recomp_rs::run_bank`]) is only a *sound* fallback if
//! it is architecturally indistinguishable from the AOT bank runner
//! ([`fn64_recomp_rs::emit_bank_runner`]). This test proves that: for each
//! synthetic bank of ordinary instructions it runs BOTH lanes over an identical
//! initial `RecompContext` + `Rdram`, then asserts the final architectural state
//! (all GPRs, HI/LO, COP0 Count/Compare, the FPU condition flag), the full RDRAM
//! image, and the returned `BlockExit` + retired instruction count are
//! byte-identical. Equivalence over the whole program set IS the correctness
//! proof.
//!
//! The AOT runner is emitted Rust; it is compiled into a host binary that links
//! this crate, so the same binary can call the emitted function AND the library
//! `run_bank` interpreter and compare them in-process. This reuses the
//! compile-and-run infrastructure proven in `tests/bank_runner.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

use fn64_recomp_rs::{emit_bank_runner, BankId, BankInput};

/// One synthetic bank plus the initial machine state to run it from. Every
/// program's words decode to ordinary integer/control/memory ops (no FPU/COP0),
/// which is exactly the slice the interpreter covers and the AOT lane can emit
/// without a host `panic!` arm.
struct Program {
    /// Distinct emitted-runner name (also the differential label).
    name: &'static str,
    bank: u64,
    vram: u32,
    words: &'static [u32],
    /// PC to begin execution at (an interior entry is deliberately allowed).
    entry: u32,
    /// Instruction budget for the single turn.
    budget: u32,
    /// `(index, value)` GPR writes applied before the run (both lanes).
    init_regs: &'static [(u8, u64)],
    /// RDRAM size in bytes for the run (small sizes make some accesses fault).
    rdram_len: usize,
    /// `(byte_offset, value)` RDRAM initializer bytes applied before the run.
    init_mem: &'static [(usize, u8)],
}

const BASE: u32 = 0x8000_1000;

/// The synthetic program corpus. Each covers a specific instruction class the
/// differential must agree on. `jr $ra` / a `jr $t` interior transfer / a
/// static `j`/`beq` provide the exit; a full RDRAM (0x8000+ bytes covering the
/// KSEG0 base) backs the memory programs so ordinary loads/stores land, while a
/// deliberately tiny RDRAM forces the fault program.
fn programs() -> Vec<Program> {
    vec![
        // Arithmetic / logical / immediate, then jr $ra (ResolveTransfer out).
        Program {
            name: "p_alu",
            bank: 0x01,
            vram: BASE,
            words: &[
                0x2402_0005, // addiu $v0,$zero,5
                0x2403_0003, // addiu $v1,$zero,3
                0x0043_2020, // add   $a0,$v0,$v1      -> 8
                0x0043_2822, // sub   $a1,$v0,$v1      -> 2
                0x0043_3024, // and   $a2,$v0,$v1      -> 1
                0x0043_3825, // or    $a3,$v0,$v1      -> 7
                0x0043_4026, // xor   $t0,$v0,$v1      -> 6
                0x0043_4827, // nor   $t1,$v0,$v1
                0x0043_502A, // slt   $t2,$v0,$v1      -> 0
                0x0062_502B, // sltu  $t2,$v1,$v0      -> 1
                0x03E0_0008, // jr    $ra
                0x0000_0000, // nop (delay)
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 0,
            init_mem: &[],
        },
        // Shifts (immediate and variable, 32- and 64-bit) then jr $ra.
        Program {
            name: "p_shift",
            bank: 0x02,
            vram: BASE,
            words: &[
                0x2402_00FF, // addiu $v0,$zero,255
                0x0002_2100, // sll   $a0,$v0,4
                0x0002_2902, // srl   $a1,$v0,4
                0x2403_FFFF, // addiu $v1,$zero,-1
                0x0003_3043, // sra   $a2,$v1,1
                0x2404_0002, // addiu $a0,$zero,2
                0x0044_3804, // sllv  $a3,$a0,$v0  (shift by $v0&31)
                0x0002_10F8, // dsll  $v0,$v0,3
                0x03E0_0008, // jr    $ra
                0x0000_0000, // nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 0,
            init_mem: &[],
        },
        // Mult/Div HI/LO, mfhi/mflo, then jr $ra.
        Program {
            name: "p_muldiv",
            bank: 0x03,
            vram: BASE,
            words: &[
                0x2402_0007, // addiu $v0,$zero,7
                0x2403_0003, // addiu $v1,$zero,3
                0x0043_0018, // mult  $v0,$v1
                0x0000_2010, // mfhi  $a0
                0x0000_2812, // mflo  $a1     -> 21
                0x0043_001A, // div   $v0,$v1
                0x0000_3010, // mfhi  $a2     -> 1
                0x0000_3812, // mflo  $a3     -> 2
                0x03E0_0008, // jr    $ra
                0x0000_0000, // nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 0,
            init_mem: &[],
        },
        // All load widths from backed RDRAM (with the ^2/^3 swizzle) then jr $ra.
        // $t0 = KSEG0 base (0xFFFFFFFF_80000000); loads read offsets 0..7.
        Program {
            name: "p_loads",
            bank: 0x04,
            vram: BASE,
            words: &[
                0x3C08_8000, // lui   $t0,0x8000       -> $t0 = 0xFFFFFFFF80000000
                0x8D02_0000, // lw    $v0,0x0($t0)
                0x9503_0004, // lhu   $v1,0x4($t0)
                0x8504_0004, // lh    $a0,0x4($t0)
                0x9105_0002, // lbu   $a1,0x2($t0)
                0x8106_0002, // lb    $a2,0x2($t0)
                0x8D07_0000, // lw    $a3,0x0($t0)
                0x03E0_0008, // jr    $ra
                0x0000_0000, // nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 64,
            init_mem: &[
                (0, 0x11),
                (1, 0x22),
                (2, 0x33),
                (3, 0x44),
                (4, 0x55),
                (5, 0x66),
                (6, 0x77),
                (7, 0x88),
            ],
        },
        // All store widths into backed RDRAM then jr $ra. The differential over
        // the whole RDRAM image proves the swizzle matches.
        Program {
            name: "p_stores",
            bank: 0x05,
            vram: BASE,
            words: &[
                0x3C08_8000, // lui   $t0,0x8000
                0x2402_ABCD, // addiu $v0,$zero,0xFFFFABCD
                0xAD02_0000, // sw    $v0,0x0($t0)
                0xA502_0008, // sh    $v0,0x8($t0)
                0xA102_000C, // sb    $v0,0xC($t0)
                0x2403_1234, // addiu $v1,$zero,0x1234
                0xAD03_0010, // sw    $v1,0x10($t0)
                0x03E0_0008, // jr    $ra
                0x0000_0000, // nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 64,
            init_mem: &[],
        },
        // Doubleword load/store (LD/SD) round trip then jr $ra.
        Program {
            name: "p_dword",
            bank: 0x06,
            vram: BASE,
            words: &[
                0x3C08_8000, // lui   $t0,0x8000
                0xDD02_0000, // ld    $v0,0x0($t0)
                0xFD02_0008, // sd    $v0,0x8($t0)
                0x03E0_0008, // jr    $ra
                0x0000_0000, // nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 64,
            init_mem: &[
                (0, 0xDE),
                (1, 0xAD),
                (2, 0xBE),
                (3, 0xEF),
                (4, 0xCA),
                (5, 0xFE),
                (6, 0xBA),
                (7, 0xBE),
            ],
        },
        // Unaligned word load/store (LWL/LWR/SWL/SWR) then jr $ra.
        Program {
            name: "p_unaligned",
            bank: 0x07,
            vram: BASE,
            words: &[
                0x3C08_8000, // lui   $t0,0x8000
                0x8902_0001, // lwl   $v0,0x1($t0)
                0x9902_0004, // lwr   $v0,0x4($t0)
                0xA903_0009, // swl   $v1,0x9($t0)
                0xB903_000C, // swr   $v1,0xC($t0)
                0x03E0_0008, // jr    $ra
                0x0000_0000, // nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000), (3, 0xFFFF_FFFF_1122_3344)],
            rdram_len: 64,
            init_mem: &[
                (0, 0x01),
                (1, 0x23),
                (2, 0x45),
                (3, 0x67),
                (4, 0x89),
                (5, 0xAB),
                (6, 0xCD),
                (7, 0xEF),
            ],
        },
        // LL/SC success then SC failure path, then jr $ra.
        Program {
            name: "p_llsc",
            bank: 0x08,
            vram: BASE,
            words: &[
                0x3C08_8000, // lui   $t0,0x8000
                0xC102_0000, // ll    $v0,0x0($t0)
                0x2442_0001, // addiu $v0,$v0,1
                0xE102_0000, // sc    $v0,0x0($t0)   -> success, $v0 = 1
                0xE103_0004, // sc    $v1,0x4($t0)   -> no reservation, $v1 = 0
                0x03E0_0008, // jr    $ra
                0x0000_0000, // nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000), (3, 0x55)],
            rdram_len: 64,
            init_mem: &[(3, 0x09)],
        },
        // Branch taken (beq) with a delay slot; the branch target is in-bank.
        Program {
            name: "p_branch_taken",
            bank: 0x09,
            vram: BASE,
            words: &[
                0x2402_0001, // 1000 addiu $v0,$zero,1
                0x1040_0002, // 1004 beq   $v0,$zero,+2 -> not taken here...
                0x2404_0007, // 1008 addiu $a0,$zero,7 (delay)
                0x2405_0009, // 100C addiu $a1,$zero,9
                0x1000_FFFB, // 1010 beq   $zero,$zero,-5 -> taken, target 0x1000
                0x2406_000B, // 1014 addiu $a2,$zero,11 (delay)
                0x03E0_0008, // 1018 jr $ra
                0x0000_0000, // 101C nop
            ],
            entry: BASE + 0x0C,
            budget: 64,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 0,
            init_mem: &[],
        },
        // Branch NOT taken (bne with equal operands falls through), delay slot
        // still runs; end with jr $ra.
        Program {
            name: "p_branch_not_taken",
            bank: 0x0A,
            vram: BASE,
            words: &[
                0x2402_0004, // addiu $v0,$zero,4
                0x1442_0002, // bne   $v0,$v0,+2   -> not taken (equal)
                0x2404_0007, // addiu $a0,$zero,7  (delay runs)
                0x2405_0009, // addiu $a1,$zero,9  (fallthrough)
                0x03E0_0008, // jr $ra
                0x0000_0000, // nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 0,
            init_mem: &[],
        },
        // Branch-likely NOT taken: the delay slot is ANNULLED (must not run).
        Program {
            name: "p_branch_likely_annul",
            bank: 0x0B,
            vram: BASE,
            words: &[
                0x2402_0004, // addiu $v0,$zero,4
                0x5440_0002, // beql  $v0,$zero,+2 -> not taken (v0!=0)
                0x2404_00FF, // addiu $a0,$zero,255 (delay: ANNULLED, must stay 0)
                0x2405_0009, // addiu $a1,$zero,9
                0x03E0_0008, // jr $ra
                0x0000_0000, // nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 0,
            init_mem: &[],
        },
        // Branch-likely taken: the delay slot DOES run.
        Program {
            name: "p_branch_likely_taken",
            bank: 0x0C,
            vram: BASE,
            words: &[
                0x1000_0002, // 1000 beq $zero,$zero,+2 (unconditional) target 100C
                0x2404_0001, // 1004 addiu $a0,$zero,1 (delay)
                0x2405_00AA, // 1008 addiu $a1,$zero,0xAA (skipped)
                0x5000_FFFD, // 100C beql $zero,$zero,-3 -> taken, target 1004
                0x2406_0007, // 1010 addiu $a2,$zero,7 (delay runs)
                0x03E0_0008, // 1014 jr $ra
                0x0000_0000, // 1018 nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 0,
            init_mem: &[],
        },
        // JAL (link) to an in-bank target, then jr $ra from the callee.
        Program {
            name: "p_jal",
            bank: 0x0D,
            vram: BASE,
            words: &[
                0x0C00_0403, // 1000 jal 0x8000100C (in-bank; $ra = fallthrough 0x1008)
                0x2404_0003, // 1004 addiu $a0,$zero,3 (delay)
                0x2405_0009, // 1008 addiu $a1,$zero,9 (fallthrough, not reached)
                0x2402_002A, // 100C addiu $v0,$zero,42
                0x03E0_0008, // 1010 jr $ra
                0x0000_0000, // 1014 nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 0,
            init_mem: &[],
        },
        // JALR computed call: $t9 holds an in-bank target; the link goes to $ra.
        Program {
            name: "p_jalr",
            bank: 0x0E,
            vram: BASE,
            words: &[
                0x0320_F809, // 1000 jalr $ra,$t9
                0x2404_0005, // 1004 addiu $a0,$zero,5 (delay)
                0x2405_0009, // 1008 addiu $a1,$zero,9 (not run)
                0x03E0_0008, // 100C jr $ra
                0x0000_0000, // 1010 nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(25, (BASE + 0x0C) as u64 as i64 as u64)],
            rdram_len: 0,
            init_mem: &[],
        },
        // JR whose delay slot OVERWRITES the source register: the snapshot must
        // win, so the transfer target is the pre-delay value.
        Program {
            name: "p_jr_snapshot",
            bank: 0x0F,
            vram: BASE,
            words: &[
                0x0100_0008, // jr    $t0
                0x2408_1234, // addiu $t0,$zero,0x1234 (delay overwrites $t0)
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(8, 0x8000_2000)],
            rdram_len: 0,
            init_mem: &[],
        },
        // Budget checkpoint: a 2-instruction budget on a straight run stops at a
        // deterministic Checkpoint without splitting a branch/delay pair.
        Program {
            name: "p_checkpoint",
            bank: 0x10,
            vram: BASE,
            words: &[
                0x2402_0001, // addiu $v0,$zero,1
                0x2442_0002, // addiu $v0,$v0,2
                0x1042_0001, // beq   $v0,$v0,+1  (would need pair)
                0x2404_0007, // addiu $a0,$zero,7 (delay)
                0x03E0_0008, // jr $ra
                0x0000_0000, // nop
            ],
            entry: BASE,
            budget: 2,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 0,
            init_mem: &[],
        },
        // Memory fault: a store to an address outside a tiny RDRAM is a typed
        // MemoryFault in BOTH lanes, with identical retired count and addr.
        Program {
            name: "p_memfault",
            bank: 0x11,
            vram: BASE,
            words: &[
                0x3C08_8000, // lui   $t0,0x8000
                0xAD02_0040, // sw    $v0,0x40($t0)  (offset 0x40 > 16-byte rdram)
                0x03E0_0008, // jr $ra
                0x0000_0000, // nop
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[(31, 0x8000_9000)],
            rdram_len: 16,
            init_mem: &[],
        },
        // Self-loop yield: `beq $zero,$zero,self` runs its delay slot and yields.
        Program {
            name: "p_selfloop",
            bank: 0x12,
            vram: BASE,
            words: &[
                0x1000_FFFF, // beq $zero,$zero,-1  (target = self)
                0x2404_0007, // addiu $a0,$zero,7 (delay)
            ],
            entry: BASE,
            budget: 64,
            init_regs: &[],
            rdram_len: 0,
            init_mem: &[],
        },
    ]
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

/// Render the `Program` struct literal (arrays of tuples) as Rust source so the
/// harness can reconstruct the identical initial state for both lanes.
fn render_program_setup(p: &Program) -> String {
    let init_regs = p
        .init_regs
        .iter()
        .map(|(i, v)| format!("({i}u8, {v:#018X}u64)"))
        .collect::<Vec<_>>()
        .join(", ");
    let init_mem = p
        .init_mem
        .iter()
        .map(|(o, v)| format!("({o}usize, {v:#04X}u8)"))
        .collect::<Vec<_>>()
        .join(", ");
    let words = p
        .words
        .iter()
        .map(|w| format!("{w:#010X}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"    check(
        "{name}",
        BankId::new({bank:#018X}),
        {vram:#010X},
        &[{words}],
        {entry:#010X},
        {budget},
        &[{init_regs}],
        {rdram_len},
        &[{init_mem}],
        {runner},
    );"#,
        name = p.name,
        bank = p.bank,
        vram = p.vram,
        entry = p.entry,
        budget = p.budget,
        rdram_len = p.rdram_len,
        runner = p.name,
    )
}

#[test]
fn interpreter_matches_aot_bank_runner_on_ordinary_programs() {
    let programs = programs();

    // Emit one AOT runner per program (its `name` is the emitted fn name).
    let mut emitted = String::new();
    for p in &programs {
        emitted.push_str(&emit_bank_runner(&BankInput {
            name: p.name,
            bank: BankId::new(p.bank),
            vram: p.vram,
            words: p.words,
        }));
        emitted.push('\n');
    }

    let checks = programs
        .iter()
        .map(render_program_setup)
        .collect::<Vec<_>>()
        .join("\n");
    let program_count = programs.len();

    // The harness builds identical initial state, runs both lanes, and asserts
    // byte-equal architectural state + Rdram + BlockExit + instruction count.
    let source = format!(
        r#"#![allow(unused_imports)]
use fn64_recomp_rs::{{
    run_bank, BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CodeCatalog, CodeSpan,
    CpuException, CpuFault, CpuFaultKind, ExecutionKey, GeneratedBankRunner, GuestPc,
    InstructionBudget, ProgramError, Rdram, RecompContext,
}};

{emitted}

type AotRunner = fn(ExecutionKey, InstructionBudget, &mut RecompContext, &mut Rdram) -> BlockRun;

/// A comparable snapshot of all *observable* architectural state. FPU register
/// bits and FCSR are private and untouched by this integer/control/memory slice,
/// so the observable set (GPRs, HI/LO, COP0 Count/Compare/Random/cond, FPU cond
/// flag) plus the full RDRAM image is the complete differential surface here.
#[derive(PartialEq, Eq, Debug)]
struct State {{
    gprs: [u64; 32],
    hi: u64,
    lo: u64,
    cop0_count: u32,
    cop0_compare: u32,
    cop0_random: u32,
    cop0_cond: bool,
    fpu_cond: bool,
    mem: Vec<u8>,
}}

fn snapshot(ctx: &RecompContext, mem: &[u8]) -> State {{
    State {{
        gprs: ctx.gprs(),
        hi: ctx.hi,
        lo: ctx.lo,
        cop0_count: ctx.cop0_count,
        cop0_compare: ctx.cop0_compare,
        cop0_random: ctx.read_cop0(1),
        cop0_cond: ctx.cop0_cond,
        fpu_cond: ctx.fpu_cond,
        mem: mem.to_vec(),
    }}
}}

fn make_ctx(init_regs: &[(u8, u64)]) -> RecompContext {{
    let mut ctx = RecompContext::new();
    for &(i, v) in init_regs {{
        ctx.set_r(i, v);
    }}
    ctx
}}

#[allow(clippy::too_many_arguments)]
fn check(
    name: &str,
    bank: BankId,
    vram: u32,
    words: &[u32],
    entry: u32,
    budget: u32,
    init_regs: &[(u8, u64)],
    rdram_len: usize,
    init_mem: &[(usize, u8)],
    aot: AotRunner,
) {{
    let key = ExecutionKey::new(bank, GuestPc::new(entry));
    let budget = InstructionBudget::new(budget).expect("budget >= 2");

    // Interpreter lane.
    let mut interp_ctx = make_ctx(init_regs);
    let mut interp_storage = vec![0u8; rdram_len];
    for &(o, v) in init_mem {{
        interp_storage[o] = v;
    }}
    let code = CodeBank::new(bank, GuestPc::new(vram), words.to_vec())
        .expect("admit contiguous synthetic bank");
    let mut catalog = CodeCatalog::new();
    catalog.register(code).expect("register bank");
    let interp_run = {{
        let mut mem = Rdram::new(&mut interp_storage);
        run_bank(&catalog, bank, key, budget, &mut interp_ctx, &mut mem)
            .expect("interpreter covers this synthetic program without an unsupported op")
    }};
    let interp_state = snapshot(&interp_ctx, &interp_storage);

    // AOT lane, identical initial state.
    let mut aot_ctx = make_ctx(init_regs);
    let mut aot_storage = vec![0u8; rdram_len];
    for &(o, v) in init_mem {{
        aot_storage[o] = v;
    }}
    let aot_run = {{
        let mut mem = Rdram::new(&mut aot_storage);
        aot(key, budget, &mut aot_ctx, &mut mem)
    }};
    let aot_state = snapshot(&aot_ctx, &aot_storage);

    assert_eq!(
        interp_run, aot_run,
        "[{{name}}] BlockRun (exit + instruction count) diverged: interp={{interp_run:?}} aot={{aot_run:?}}",
    );
    assert_eq!(
        interp_state, aot_state,
        "[{{name}}] architectural state diverged between interpreter and AOT lanes",
    );
}}

fn main() {{
{checks}
    println!("differential ok: {program_count} programs cross-checked byte-identical");
}}
"#
    );

    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let source_path = out_dir.join("fn64_interp_differential_gate.rs");
    let binary_path = out_dir.join("fn64_interp_differential_gate");
    std::fs::write(&source_path, source).expect("write differential harness source");

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
        .expect("invoke rustc for differential harness");
    assert!(
        compile.status.success(),
        "differential harness did not compile:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&binary_path)
        .output()
        .expect("run differential harness");
    assert!(
        run.status.success(),
        "differential harness failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains(&format!(
            "differential ok: {program_count} programs cross-checked byte-identical"
        )),
        "harness did not confirm all programs cross-checked: {stdout}"
    );
}
