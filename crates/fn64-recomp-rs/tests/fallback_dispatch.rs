//! Differential + safety gate for the `dynamic_mips` interpreter fallback wired
//! behind the block dispatcher.
//!
//! [`tests/interp_differential.rs`](../interp_differential.rs) proves the
//! interpreter and the AOT bank runner leave byte-identical state for a single
//! turn. This gate proves the *dispatcher* integration: a `FallbackProgram`
//! that registers some banks as AOT and some as `dynamic_mips` is driven through
//! `dispatch_until_boundary`, and
//!
//! - a block that runs AOT and the same block that runs interpreted leave
//!   byte-identical architectural state (differential through the dispatcher);
//! - a transfer from an AOT-admitted block into an interpreter-fallback bank and
//!   back works and keeps bank identity;
//! - an `UnmappedPc` / data-hole address STILL faults typed with the fallback
//!   installed (the load-bearing safety property);
//! - a budget checkpoint and a memory fault behave identically in the fallback
//!   lane.
//!
//! The AOT lane is emitted Rust, so the whole thing is compiled into a host
//! binary that links this crate (reusing the `tests/bank_runner.rs`
//! infrastructure) and asserts in-process.

use std::path::{Path, PathBuf};
use std::process::Command;

use fn64_recomp_rs::{emit_bank_runner, BankId, BankInput};

const A_VA: u32 = 0x8000_1000;
const B_VA: u32 = 0x8000_2000;

// Bank A (AOT): compute in $v0, then `j` into bank B's VA. The static jump
// target is outside bank A's admitted interval, so it exits as a
// ResolveTransfer that the dispatcher's resolver maps to bank B, keeping bank
// identity across the AOT -> dynamic_mips boundary.
const A_WORDS: [u32; 4] = [
    0x2402_0005, // addiu $v0,$zero,5
    0x2442_0003, // addiu $v0,$v0,3     -> 8
    0x0800_0800, // j     0x80002000    (into bank B)
    0x2404_0007, // addiu $a0,$zero,7   (delay)
];

// Bank B (dynamic_mips fallback): consume $v0, then `jr $ra` back out. Runs on
// the interpreter, but the dispatcher cannot tell — same BlockExit contract.
const B_WORDS: [u32; 3] = [
    0x2445_0001, // addiu $a1,$v0,1     -> 9
    0x03E0_0008, // jr    $ra
    0x0000_0000, // nop
];

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

fn compile_and_run(emitted: &str, main_body: &str) -> String {
    let source = format!(
        r#"#![allow(unused_imports)]
use fn64_recomp_rs::{{
    dispatch_until_boundary, run_bank, BankId, BlockExit, BlockProgram, BlockRun, BlockRunner,
    CodeBank, CodeCatalog, CodeSpan, CpuException, CpuFault, CpuFaultKind, DispatchRun, EvidenceClass,
    ExecutionKey, FallbackProgram, GeneratedBankRunner, GuestPc, InstructionBudget, ProgramError,
    Rdram, RecompContext,
}};

{emitted}

fn main() {{
{main_body}
    println!("fallback dispatch ok");
}}
"#
    );

    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    // A process-unique key: `SystemTime` alone collides when two harness tests
    // in the same binary run in parallel and land in the same instant, so a
    // monotonic counter disambiguates them deterministically.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let key = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let source_path = out_dir.join(format!("fn64_fallback_dispatch_{key}.rs"));
    let binary_path = out_dir.join(format!("fn64_fallback_dispatch_{key}"));
    std::fs::write(&source_path, source).expect("write fallback dispatch source");

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
        .expect("invoke rustc for fallback dispatch gate");
    assert!(
        compile.status.success(),
        "fallback dispatch gate did not compile:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&binary_path)
        .output()
        .expect("run fallback dispatch gate");
    assert!(
        run.status.success(),
        "fallback dispatch gate failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).into_owned()
}

/// The shared emitted-runner prelude: an AOT runner for bank A and an AOT runner
/// for bank B (used to prove that an interpreted B leaves the SAME state an AOT
/// B would). The dispatch differential registers B as `dynamic_mips`.
fn emitted_runners() -> String {
    let a = emit_bank_runner(&BankInput {
        name: "aot_bank_a",
        bank: BankId::new(0xAA),
        vram: A_VA,
        words: &A_WORDS,
    });
    let b = emit_bank_runner(&BankInput {
        name: "aot_bank_b",
        bank: BankId::new(0xBB),
        vram: B_VA,
        words: &B_WORDS,
    });
    format!("{a}\n{b}\n")
}

#[test]
fn aot_to_dynamic_mips_transfer_and_back_keeps_bank_identity_and_matches_all_aot() {
    let body = format!(
        r#"
    let a_id = BankId::new(0xAA);
    let b_id = BankId::new(0xBB);
    let a_code = || CodeBank::new(a_id, GuestPc::new({a_va:#010X}), vec![{a_words}]).unwrap();
    let b_code = || CodeBank::new(b_id, GuestPc::new({b_va:#010X}), vec![{b_words}]).unwrap();
    let entry = ExecutionKey::new(a_id, GuestPc::new({a_va:#010X}));
    let budget = InstructionBudget::new(64).unwrap();

    // Resolver: bank A's static `j` into bank B's VA is a ResolveTransfer; the
    // active mapping layer resolves it to bank B, and bank B's `jr $ra` exits
    // the whole program via ResolveTransfer to $ra (out of both banks).
    let resolve = |_src: BankId, target: GuestPc| -> Result<ExecutionKey, CpuFault> {{
        if target == GuestPc::new({b_va:#010X}) {{
            Ok(ExecutionKey::new(b_id, target))
        }} else {{
            // $ra target ({ra:#010X}) is outside both banks: unknown bank fault
            // ends the dispatch loop (the program has returned to its caller).
            Err(CpuFault {{ at: ExecutionKey::new(b_id, target), kind: CpuFaultKind::UnknownBank }})
        }}
    }};

    // --- Mixed lane: A is AOT, B is dynamic_mips (interpreter) ---
    let mut mixed = FallbackProgram::new();
    mixed.register_aot(a_code(), GeneratedBankRunner::new(a_id, aot_bank_a)).unwrap();
    mixed.register_dynamic_mips(b_code()).unwrap();
    assert_eq!(mixed.evidence_class(a_id), Some(EvidenceClass::BlockAot));
    assert_eq!(mixed.evidence_class(b_id), Some(EvidenceClass::DynamicMips));

    let mut mixed_ctx = RecompContext::new();
    mixed_ctx.set_r(31, {ra:#010X});
    let mixed_run = {{
        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut runner = mixed.runner(&mut mixed_ctx, &mut mem);
        let mut r = resolve;
        dispatch_until_boundary(entry, budget, &mut runner, &mut r).unwrap()
    }};

    // --- All-AOT lane: both A and B AOT, identical inputs ---
    let mut all_aot = FallbackProgram::new();
    all_aot.register_aot(a_code(), GeneratedBankRunner::new(a_id, aot_bank_a)).unwrap();
    all_aot.register_aot(b_code(), GeneratedBankRunner::new(b_id, aot_bank_b)).unwrap();
    assert_eq!(all_aot.evidence_class(b_id), Some(EvidenceClass::BlockAot));

    let mut aot_ctx = RecompContext::new();
    aot_ctx.set_r(31, {ra:#010X});
    let aot_run = {{
        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut runner = all_aot.runner(&mut aot_ctx, &mut mem);
        let mut r = resolve;
        dispatch_until_boundary(entry, budget, &mut runner, &mut r).unwrap()
    }};

    // The whole dispatched run is architecturally indistinguishable whether B
    // ran AOT or interpreted.
    assert_eq!(mixed_run, aot_run, "mixed vs all-AOT DispatchRun diverged");
    assert_eq!(mixed_ctx.gprs(), aot_ctx.gprs(), "final GPRs diverged");

    // Bank identity was kept across the AOT -> dynamic_mips boundary: $v0 was
    // computed in A (AOT) and consumed in B (interpreter).
    assert_eq!(mixed_ctx.r_u32(2), 8, "$v0 computed in bank A");
    assert_eq!(mixed_ctx.r_u32(5), 9, "$a1 computed in bank B from A's $v0");
    // The program exited via ResolveTransfer to $ra, which the resolver
    // rejected as an unknown bank (returned to caller): a typed fault exit.
    assert!(matches!(
        mixed_run.exit,
        BlockExit::Fault(CpuFault {{ kind: CpuFaultKind::UnknownBank, .. }})
    ));
"#,
        a_va = A_VA,
        b_va = B_VA,
        ra = 0x8000_9000u32,
        a_words = A_WORDS
            .iter()
            .map(|w| format!("{w:#010X}"))
            .collect::<Vec<_>>()
            .join(", "),
        b_words = B_WORDS
            .iter()
            .map(|w| format!("{w:#010X}"))
            .collect::<Vec<_>>()
            .join(", "),
    );
    let stdout = compile_and_run(&emitted_runners(), &body);
    assert!(stdout.contains("fallback dispatch ok"), "{stdout}");
}

#[test]
fn a_data_hole_still_faults_typed_with_the_fallback_installed() {
    // Bank B registered as dynamic_mips as two disjoint spans with a hole at
    // B_VA+4. Dispatching directly into the hole must fault typed (UnmappedPc),
    // never run the interpreter on the data byte.
    let body = format!(
        r#"
    let b_id = BankId::new(0xBB);
    let sparse = CodeBank::from_spans(
        b_id,
        vec![
            CodeSpan::new(b_id, GuestPc::new({b_va:#010X}), vec![0x2445_0001]).unwrap(),
            CodeSpan::new(b_id, GuestPc::new({hole_end:#010X}), vec![0x03E0_0008, 0x0000_0000]).unwrap(),
        ],
    ).unwrap();
    let mut program = FallbackProgram::new();
    program.register_dynamic_mips(sparse).unwrap();

    let mut ctx = RecompContext::new();
    let mut storage = vec![0u8; 16];
    let mut mem = Rdram::new(&mut storage);
    // Run() straight at the hole: admission front-runs the interpreter.
    let run = program.run(
        ExecutionKey::new(b_id, GuestPc::new({hole:#010X})),
        InstructionBudget::new(8).unwrap(),
        &mut ctx,
        &mut mem,
    );
    assert!(matches!(
        run.exit,
        BlockExit::Fault(CpuFault {{ kind: CpuFaultKind::UnmappedPc {{ .. }}, .. }})
    ), "hole must fault typed: {{run:?}}");
    assert_eq!(run.instructions, 0);
    assert_eq!(ctx.r_u32(5), 0, "no interpreter step ran for the data hole");

    // And through the dispatcher: a resolver that hands the hole PC back must
    // still surface the typed fault, not execute data as code.
    let mut runner = program.runner(&mut ctx, &mut mem);
    let mut r = |_src: BankId, _t: GuestPc| -> Result<ExecutionKey, CpuFault> {{ unreachable!() }};
    let dispatched = dispatch_until_boundary(
        ExecutionKey::new(b_id, GuestPc::new({hole:#010X})),
        InstructionBudget::new(8).unwrap(),
        &mut runner,
        &mut r,
    ).unwrap();
    assert!(matches!(
        dispatched.exit,
        BlockExit::Fault(CpuFault {{ kind: CpuFaultKind::UnmappedPc {{ .. }}, .. }})
    ), "dispatched hole must fault typed: {{dispatched:?}}");
"#,
        b_va = B_VA,
        hole = B_VA + 4,
        hole_end = B_VA + 8,
    );
    let stdout = compile_and_run(&emitted_runners(), &body);
    assert!(stdout.contains("fallback dispatch ok"), "{stdout}");
}

#[test]
fn budget_checkpoint_and_memory_fault_are_identical_across_lanes() {
    // A straight run of three ops with a 2-instruction budget must Checkpoint at
    // the same PC in both lanes; a store outside a tiny RDRAM must MemoryFault
    // identically in both lanes.
    let checkpoint_words = [
        0x2402_0001u32, // addiu $v0,$zero,1
        0x2442_0002,    // addiu $v0,$v0,2
        0x03E0_0008,    // jr $ra
        0x0000_0000,    // nop
    ];
    let fault_words = [
        0x3C08_8000u32, // lui $t0,0x8000
        0xAD02_0040,    // sw $v0,0x40($t0)   (offset 0x40 > 16-byte rdram)
        0x03E0_0008,    // jr $ra
        0x0000_0000,    // nop
    ];
    let emitted = {
        let cp = emit_bank_runner(&BankInput {
            name: "aot_checkpoint",
            bank: BankId::new(0xC0),
            vram: A_VA,
            words: &checkpoint_words,
        });
        let ft = emit_bank_runner(&BankInput {
            name: "aot_fault",
            bank: BankId::new(0xF0),
            vram: A_VA,
            words: &fault_words,
        });
        format!("{cp}\n{ft}\n")
    };
    let body = format!(
        r#"
    let cp_id = BankId::new(0xC0);
    let ft_id = BankId::new(0xF0);
    let cp_words = || vec![{cp_words}];
    let ft_words = || vec![{ft_words}];

    // Checkpoint: budget 2 on a 3-straight-op run stops at the same PC.
    let cp_entry = ExecutionKey::new(cp_id, GuestPc::new({a_va:#010X}));
    let cp_budget = InstructionBudget::new(2).unwrap();

    let mut interp_cp = FallbackProgram::new();
    interp_cp.register_dynamic_mips(CodeBank::new(cp_id, GuestPc::new({a_va:#010X}), cp_words()).unwrap()).unwrap();
    let mut aot_cp = FallbackProgram::new();
    aot_cp.register_aot(CodeBank::new(cp_id, GuestPc::new({a_va:#010X}), cp_words()).unwrap(), GeneratedBankRunner::new(cp_id, aot_checkpoint)).unwrap();

    let (mut ictx, mut actx) = (RecompContext::new(), RecompContext::new());
    let irun = {{ let mut s = vec![0u8; 16]; let mut m = Rdram::new(&mut s); interp_cp.run(cp_entry, cp_budget, &mut ictx, &mut m) }};
    let arun = {{ let mut s = vec![0u8; 16]; let mut m = Rdram::new(&mut s); aot_cp.run(cp_entry, cp_budget, &mut actx, &mut m) }};
    assert_eq!(irun, arun, "checkpoint BlockRun diverged: interp={{irun:?}} aot={{arun:?}}");
    assert_eq!(ictx.gprs(), actx.gprs(), "checkpoint GPRs diverged");
    assert!(matches!(irun.exit, BlockExit::Checkpoint(_)), "expected checkpoint: {{irun:?}}");

    // Memory fault: identical typed fault, retired count, addr, register state.
    let ft_entry = ExecutionKey::new(ft_id, GuestPc::new({a_va:#010X}));
    let ft_budget = InstructionBudget::new(64).unwrap();

    let mut interp_ft = FallbackProgram::new();
    interp_ft.register_dynamic_mips(CodeBank::new(ft_id, GuestPc::new({a_va:#010X}), ft_words()).unwrap()).unwrap();
    let mut aot_ft = FallbackProgram::new();
    aot_ft.register_aot(CodeBank::new(ft_id, GuestPc::new({a_va:#010X}), ft_words()).unwrap(), GeneratedBankRunner::new(ft_id, aot_fault)).unwrap();

    let (mut ictx2, mut actx2) = (RecompContext::new(), RecompContext::new());
    let irun2 = {{ let mut s = vec![0u8; 16]; let mut m = Rdram::new(&mut s); interp_ft.run(ft_entry, ft_budget, &mut ictx2, &mut m) }};
    let arun2 = {{ let mut s = vec![0u8; 16]; let mut m = Rdram::new(&mut s); aot_ft.run(ft_entry, ft_budget, &mut actx2, &mut m) }};
    assert_eq!(irun2, arun2, "memfault BlockRun diverged: interp={{irun2:?}} aot={{arun2:?}}");
    assert_eq!(ictx2.gprs(), actx2.gprs(), "memfault GPRs diverged");
    match irun2.exit {{
        BlockExit::Fault(CpuFault {{ kind: CpuFaultKind::MemoryFault {{ addr }}, .. }}) => {{
            assert_eq!(addr, 0xFFFF_FFFF_8000_0040u64);
            assert_eq!(irun2.instructions, 1, "only the LUI retired before the fault");
        }}
        other => panic!("expected typed MemoryFault, got {{other:?}}"),
    }}
"#,
        a_va = A_VA,
        cp_words = checkpoint_words
            .iter()
            .map(|w| format!("{w:#010X}"))
            .collect::<Vec<_>>()
            .join(", "),
        ft_words = fault_words
            .iter()
            .map(|w| format!("{w:#010X}"))
            .collect::<Vec<_>>()
            .join(", "),
    );
    let stdout = compile_and_run(&emitted, &body);
    assert!(stdout.contains("fallback dispatch ok"), "{stdout}");
}
