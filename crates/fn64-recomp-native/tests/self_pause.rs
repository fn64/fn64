//! Regression coverage for N64Recomp's cooperative self-loop rule.

use fn64_recomp_native::{emit_function, FuncInput};

#[test]
fn unconditional_branch_to_self_yields_before_repeating() {
    // `beq $zero,$zero,-1` is the assembler pseudo-instruction `b .`.
    let words = [0x1000_FFFF, 0x0000_0000];
    let emitted = emit_function(&FuncInput {
        name: "idle_loop",
        vram: 0x8000_1000,
        words: &words,
    });

    assert!(
        emitted.contains("pause_self();"),
        "self-loop must cooperatively yield:\n{emitted}"
    );
    assert!(
        emitted.contains("pc = 0x80001000; continue 'run;"),
        "resuming the coroutine must repeat the loop:\n{emitted}"
    );
}

#[test]
fn computed_jump_inside_function_stays_in_local_dispatcher() {
    // `jr $t0; nop` is enough to lock the lowering decision. The runtime
    // value decides whether this is a local jump-table label or an external
    // tail call.
    let words = [0x0100_0008, 0x0000_0000];
    let emitted = emit_function(&FuncInput {
        name: "jump_table",
        vram: 0x8000_1000,
        words: &words,
    });

    assert!(
        emitted.contains(
            "if _target >= 0x80001000 && _target < 0x80001008 { pc = _target; continue 'run; }"
        ),
        "intra-function computed targets must not enter whole-module lookup:\n{emitted}"
    );
    assert!(emitted.contains("lookup(_target)(ctx, mem); return;"));
}

#[test]
fn branch_targeting_another_branch_delay_slot_gets_a_block() {
    // 1000: b 100c; 1004: nop; 1008: bne zero,zero,1010;
    // 100c: lui t2,0x8012 (both the second branch's delay and first target).
    let words = [
        0x1000_0002,
        0x0000_0000,
        0x1400_0001,
        0x3C0A_8012,
        0x03E0_0008,
        0x0000_0000,
    ];
    let emitted = emit_function(&FuncInput {
        name: "delay_target",
        vram: 0x8000_1000,
        words: &words,
    });

    assert!(
        emitted.contains("        0x8000100C => {"),
        "a reachable delay-slot address needs its own local block:\n{emitted}"
    );
    assert!(emitted.contains("pc = if _take { 0x8000100C }"));
}
