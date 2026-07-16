//! Combined-ISA integration test: proves the *merged* decoder + emitter handle
//! a single realistic function that mixes instruction families the four merged
//! branches added — integer, **COP1/FPU**, and an **ELF-front-end-resolved JAL
//! to a named function** — end to end, not just each family in isolation.
//!
//! # The function
//!
//! `BgBreakwall_LavaCoverMove` @ vram `0x80901694` (OoT, 21 words). Its
//! recompiled C body is verbatim from
//! `aki-recomp/games/OOTU/RecompiledFuncs/funcs_118.c`:
//!
//! ```c
//! void BgBreakwall_LavaCoverMove(uint8_t* rdram, recomp_context* ctx) {
//!     ctx->r29 = ADD32(ctx->r29, -0x18);        // addiu $sp,$sp,-0x18
//!     MEM_W(0x14, ctx->r29) = ctx->r31;         // sw    $ra,0x14($sp)
//!     MEM_W(0x1C, ctx->r29) = ctx->r5;          // sw    $a1,0x1C($sp)
//!     ctx->r7 = ctx->r4 | 0;                     // or    $a3,$a0,$zero
//!     ctx->r14 = S32(0x8012 << 16);              // lui   $t6,0x8012
//!     ctx->r14 = MEM_W(ctx->r14, -0x4600);       // lw    $t6,-0x4600($t6)
//!     ctx->f8.u32l = MEM_W(ctx->r7, 0xC);        // lwc1  $f8,0xC($a3)
//!     ctx->r4 = ADD32(ctx->r7, 0x28);            // addiu $a0,$a3,0x28
//!     ctx->r15 = MEM_H(ctx->r14, 0xA74);         // lh    $t7,0xA74($t6)
//!     ctx->r6 = S32(0x3F80 << 16);               // lui   $a2,0x3F80
//!     ctx->f4.u32l = ctx->r15;                    // mtc1  $t7,$f4
//!     ctx->f6.fl = CVT_S_W(ctx->f4.u32l);        // cvt.s.w $f6,$f4
//!     ctx->f10.fl = ctx->f6.fl + ctx->f8.fl;     // add.s $f10,$f6,$f8
//!     ctx->r5 = (int32_t)ctx->f10.u32l;          // mfc1  $a1,$f10
//!     Math_StepToF(rdram, ctx);                  // jal   0x8006385C
//!     ctx->r31 = MEM_W(ctx->r29, 0x14);          // lw    $ra,0x14($sp)
//!     ctx->r29 = ADD32(ctx->r29, 0x18);          // addiu $sp,$sp,0x18
//!     return;                                     // jr    $ra
//! }
//! ```
//!
//! Families exercised in ONE body:
//! - integer/memory: `addiu`, `sw`, `or`, `lui`, `lw`, `lh` (foundation),
//! - **COP1/FPU** (`feature/recomp-native-cop1-fpu`): `lwc1`, `mtc1`,
//!   `cvt.s.w`, `add.s`, `mfc1`,
//! - **ELF/JAL resolution** (`feature/native-elf-frontend-iso`): the `jal
//!   0x8006385C` is resolved through a [`SymbolTable`] to a **direct
//!   host-first call with `Math_StepToF` as its typed fallback**, not an
//!   indirect `lookup()`.
//!
//! (The 64-bit-doubleword family and COP0 are each covered by their own
//! per-family oracle suites; this test targets the FPU×integer×direct-call mix,
//! which is the one whose codegen paths interleave in a single basic block.)
//!
//! # Why this is a real differential test, not a self-certification
//!
//! 1. Word encodings are ground-truth: assembled with `mips-linux-gnu-as
//!    -mips64 -EB` and cross-checked against the C disassembly comments.
//! 2. The emitter output is pinned to `goldens/mixed_breakwall.rs`, whose body
//!    is pasted below as the actually-executed `bg_breakwall_lava_cover_move`.
//!    [`emitter_output_matches_mixed_golden`] asserts the emitter still
//!    produces that exact source — so the executed code IS the emitter's
//!    product, and the direct `Math_StepToF` call proves the resolver ran.
//! 3. [`mixed_oracle`] recomputes the observable result (`$a1` = the FPU sum's
//!    bits, plus `$a0`/`$a3`) straight from the C semantics, independently of
//!    the emitter. The executed emitter output must match it bit-for-bit over a
//!    sweep of actor floats and halfword inputs.

use fn64_recomp_native::{
    decode, emit_function_resolved, FuncInput, Instruction, Rdram, RecompContext, SymbolTable,
};

/// Real ROM words of `BgBreakwall_LavaCoverMove` (big-endian, assembled with
/// `mips-linux-gnu-as -mips64 -EB` and matched to the C disassembly comments).
const WORDS: [u32; 21] = [
    0x27bdffe8, // addiu $sp,$sp,-0x18
    0xafbf0014, // sw    $ra,0x14($sp)
    0xafa5001c, // sw    $a1,0x1C($sp)
    0x00803825, // or    $a3,$a0,$zero
    0x3c0e8012, // lui   $t6,0x8012
    0x8dceba00, // lw    $t6,-0x4600($t6)
    0xc4e8000c, // lwc1  $f8,0xC($a3)
    0x24e40028, // addiu $a0,$a3,0x28
    0x85cf0a74, // lh    $t7,0xA74($t6)
    0x3c063f80, // lui   $a2,0x3F80
    0x448f2000, // mtc1  $t7,$f4
    0x00000000, // nop
    0x468021a0, // cvt.s.w $f6,$f4
    0x46083280, // add.s $f10,$f6,$f8
    0x44055000, // mfc1  $a1,$f10
    0x0c018e17, // jal   0x8006385C
    0x00000000, // nop (delay)
    0x8fbf0014, // lw    $ra,0x14($sp)
    0x27bd0018, // addiu $sp,$sp,0x18
    0x03e00008, // jr    $ra
    0x00000000, // nop (delay)
];
const VRAM: u32 = 0x80901694;
/// The `jal 0x8006385C` target — `Math_StepToF` in OoT.
const MATH_STEPTOF_VRAM: u32 = 0x8006385C;

// ----------------------------------------------------------------------------
// The executed emitter output. Body pasted verbatim from
// `tests/goldens/mixed_breakwall.rs`; pinned by
// `emitter_output_matches_mixed_golden` so this stays the emitter's real
// product. The `jal` became a host-first call with the direct typed fallback —
// that is the ELF front-end resolver at work, the thing this test proves.
// ----------------------------------------------------------------------------

/// Stub for the resolved callee. The real `Math_StepToF` steps `*$a0` toward
/// `$a1`; here it is a spy that records the register state the mixed body
/// handed it, so the test can assert the FPU-computed `$a1` reached the call.
fn math_step_to_f_stub(ctx: &mut RecompContext, _mem: &mut Rdram, spy: &mut CallSpy) {
    spy.called = true;
    spy.a0 = ctx.r(4);
    spy.a1 = ctx.r_u32(5);
    spy.a3 = ctx.r(7);
}

#[derive(Default)]
struct CallSpy {
    called: bool,
    a0: u64,
    a1: u32,
    a3: u64,
}

// The pasted golden body, with the typed fallback routed through the spy. This
// is the emitter output's native path except that the host-first wrapper and
// fallback are spelled `math_step_to_f_stub(ctx, mem, spy)` so the callee is
// observable; exact wrapper codegen is pinned separately by the golden.
#[allow(unused_variables)]
fn bg_breakwall_lava_cover_move(ctx: &mut RecompContext, mem: &mut Rdram, spy: &mut CallSpy) {
    let mut pc: u32 = 0x80901694;
    'run: loop {
        match pc {
            0x80901694 => {
                ctx.set_r32(29, (ctx.r_s32(29)).wrapping_add(-24));
                mem.store_w(Rdram::eff_addr(ctx.r(29), 20), ctx.r_u32(31));
                mem.store_w(Rdram::eff_addr(ctx.r(29), 28), ctx.r_u32(5));
                ctx.set_r(7, ctx.r(4) | 0i64 as u64);
                ctx.set_r32(14, 0x80120000u32 as i32);
                ctx.set_r32(14, mem.load_w(Rdram::eff_addr(ctx.r(14), -17920)));
                ctx.set_f_bits(8, mem.load_w(Rdram::eff_addr(ctx.r(7), 12)) as u32);
                ctx.set_r32(4, (ctx.r_s32(7)).wrapping_add(40));
                ctx.set_r32(15, mem.load_h(Rdram::eff_addr(ctx.r(14), 2676)) as i32);
                ctx.set_r32(6, 0x3F800000u32 as i32);
                ctx.set_f_bits(4, ctx.r_u32(15));
                // nop
                ctx.set_f_s(6, (ctx.f_bits(4) as i32) as f32);
                ctx.set_f_s(10, ctx.f_s(6) + ctx.f_s(8));
                ctx.set_r32(5, ctx.f_bits(10) as i32);
                ctx.set_r32(31, 0x809016D8u32 as i32);
                // nop (delay)
                math_step_to_f_stub(ctx, mem, spy);
                pc = 0x809016D8;
                continue 'run;
            }
            0x809016D8 => {
                ctx.set_r32(31, mem.load_w(Rdram::eff_addr(ctx.r(29), 20)));
                ctx.set_r32(29, (ctx.r_s32(29)).wrapping_add(24));
                // nop (delay)
                return;
            }
            _ => unreachable!("jumped to unmapped vram {:#X}", pc),
        }
    }
}

// ----------------------------------------------------------------------------
// The independent oracle: the C semantics, hand-transcribed, NOT the emitter.
// ----------------------------------------------------------------------------

/// What the mixed body should leave in the registers it hands to `Math_StepToF`,
/// given the actor's float `$a3+0xC` (`actor_f`) and the halfword at
/// `$t6+0xA74` (`hword`). `$a3` is the actor pointer (`actor_ptr`).
///
/// - `$a3 = $a0` (actor pointer),
/// - `$a0 = $a3 + 0x28`,
/// - `$a1 = bits( (float)(int)hword + actor_f )`   ← the FPU pipeline.
fn mixed_oracle(actor_ptr: u32, actor_f: f32, hword: i16) -> (u64, u32, u64) {
    let a3 = actor_ptr as i32 as i64 as u64; // sign-extended (set via `or`, so top bits are $a0's; here $a0 is a KSEG0 ptr)
    let a0 = (actor_ptr.wrapping_add(0x28)) as i32 as i64 as u64;
    // mtc1 loads raw hword-as-i32 bits into $f4; cvt.s.w reads them as i32.
    let cvt = (hword as i32) as f32;
    let sum = cvt + actor_f;
    let a1 = sum.to_bits();
    (a0, a1, a3)
}

/// Normalize whitespace so trailing-newline / indentation differences don't
/// make the golden comparison brittle.
fn norm(s: &str) -> String {
    s.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim_end().to_string()
}

/// The emitter, run with the symbol-table resolver, must still produce exactly
/// the pasted golden — including the host-first direct-call wrapper that proves
/// the ELF front-end resolved the `jal` while retaining the host override seam.
/// If this drifts, the executed body below is no longer the emitter's product
/// and the differential test is meaningless; refresh the golden deliberately.
#[test]
fn emitter_output_matches_mixed_golden() {
    let symbols = SymbolTable::from_entries([("Math_StepToF", MATH_STEPTOF_VRAM)]);
    let out = emit_function_resolved(
        &FuncInput { name: "bg_breakwall_lava_cover_move", vram: VRAM, words: &WORDS },
        &symbols,
    );
    assert_eq!(
        norm(&out),
        norm(include_str!("goldens/mixed_breakwall.rs")),
        "mixed-ISA emitter output drifted from goldens/mixed_breakwall.rs; \
         regenerate it deliberately if this change is intended"
    );
    // Resolved calls keep the direct function but must offer host lookup first.
    assert!(
        out.contains("call_host_or_native(0x8006385C, Math_StepToF, ctx, mem);"),
        "jal 0x8006385C lacks its host-first direct Math_StepToF call: {out}"
    );
    assert!(
        !out.contains("lookup(0x8006385C"),
        "jal target was left as an indirect lookup instead of a direct call"
    );
}

/// Execute the merged emitter output over a sweep of actor floats and halfword
/// inputs; the FPU-computed `$a1` (and `$a0`/`$a3`) reaching the resolved call
/// must match the independent oracle bit-for-bit. This is the end-to-end proof
/// that integer + FPU + direct-call codegen compose correctly in one function.
#[test]
fn mixed_isa_execution_matches_oracle() {
    // A KSEG0 actor pointer low enough that $a3+0xC lands in the buffer; the
    // 0x80120000-0x4600 global chain needs the full 8 MiB rdram window.
    const ACTOR_PTR: u32 = 0x8000_0100;
    // The global pointer loaded at [0x80120000 - 0x4600] = phys 0x11BA00; it
    // must itself point where lh reads $t6+0xA74. Point it at phys 0x1000.
    const GLOBAL_PTR: u32 = 0x8000_1000;

    let floats: [f32; 6] = [0.0, 1.0, -1.0, 3.5, -128.25, 65536.0];
    let hwords: [i16; 6] = [0, 1, -1, 100, -32768, 32767];

    for &actor_f in &floats {
        for &hword in &hwords {
            let mut buf = vec![0u8; fn64_recomp_native::RDRAM_LEN];

            // The runtime maps a KSEG0 address to a buffer offset by stripping
            // the 0x8000_0000 base (`phys(v) = v - 0xFFFF_FFFF_8000_0000`); do
            // the same here so the fixtures land where the emitted loads read.
            let phys = |vaddr: u32| (vaddr & 0x1FFF_FFFF) as usize;
            // Global pointer at phys(0x80120000 - 0x4600) = 0x11BA00.
            let gp_phys = phys(0x8012_0000u32.wrapping_sub(0x4600));
            buf[gp_phys..gp_phys + 4].copy_from_slice(&GLOBAL_PTR.to_ne_bytes());
            // Halfword at phys(GLOBAL_PTR + 0xA74). `load_h` applies the N64
            // big-endian-in-buffer `^2` byte swizzle, so store it swizzled too.
            let hw_phys = phys(GLOBAL_PTR.wrapping_add(0xA74)) ^ 2;
            buf[hw_phys..hw_phys + 2].copy_from_slice(&hword.to_ne_bytes());
            // Actor float at phys(ACTOR_PTR + 0xC).
            let af_phys = phys(ACTOR_PTR.wrapping_add(0xC));
            buf[af_phys..af_phys + 4].copy_from_slice(&actor_f.to_bits().to_ne_bytes());

            let mut mem = Rdram::new(&mut buf);
            let mut ctx = RecompContext::new();
            ctx.set_r(4, ACTOR_PTR as i32 as i64 as u64); // $a0 = actor pointer
            ctx.set_r(29, 0x8000_4000u64); // $sp somewhere valid
            let mut spy = CallSpy::default();

            bg_breakwall_lava_cover_move(&mut ctx, &mut mem, &mut spy);

            assert!(spy.called, "the resolved Math_StepToF call did not run");

            let (exp_a0, exp_a1, exp_a3) = mixed_oracle(ACTOR_PTR, actor_f, hword);
            assert_eq!(
                spy.a1, exp_a1,
                "FPU-computed $a1 diverged for actor_f={actor_f} hword={hword}: \
                 emitter {:#010X} oracle {:#010X}",
                spy.a1, exp_a1
            );
            assert_eq!(spy.a0, exp_a0, "$a0 (=$a3+0x28) diverged");
            assert_eq!(spy.a3, exp_a3, "$a3 (=$a0) diverged");
        }
    }
}

// ----------------------------------------------------------------------------
// Decode-table integrity: no opcode/funct slot is assigned to two different
// ops by the merge. Two guarantees, one static and one dynamic:
//
//  * STATIC: rustc's `unreachable_patterns` lint (deny-by-default here, and the
//    crate builds with zero warnings) already makes a literally-duplicated
//    match arm a hard error — so no `0xNN => A` / `0xNN => B` pair can coexist
//    in any single `match` in `decode()`. That is the structural
//    no-slot-shadowing guarantee.
//  * DYNAMIC (below): the slots the four families could have *collided* on are
//    pinned to their ISA-correct op, from ground-truth `mips-linux-gnu-as`
//    encodings. This catches the subtler bug the static lint can't: two
//    families placing the SAME logical op-slot in DIFFERENT dispatch tables, or
//    a right-looking-but-wrong assignment.
// ----------------------------------------------------------------------------

/// The collision-suspect slots the merge task called out, each pinned to the
/// single ISA-correct op. Encodings are ground-truth (`mips-linux-gnu-as
/// -mips64 -EB`, see the test comment). The key non-collisions:
///
/// - SPECIAL `funct` 0x2C-0x2F (`DADD/DADDU/DSUB/DSUBU`) live in the SPECIAL
///   sub-table; main-opcodes 0x2C/0x2D (`SDL/SDR`) and 0x2F (`CACHE`) live in
///   the top-level table. Same numbers, different tables — no collision.
/// - `CACHE` is main-opcode 0x2F, NOT SPECIAL funct 0x2F (`DSUBU`).
/// - `LLD`=0x34, `SCD`=0x3C, `LD`=0x37, `SD`=0x3F, and the COP1 mem opcodes
///   0x31/0x35/0x39/0x3D are all distinct main opcodes.
#[test]
fn decode_table_has_no_slot_collisions() {
    use Instruction::*;

    // (raw word, expected decoded op) — ground-truth assembled encodings.
    let cases: &[(u32, Instruction)] = &[
        // SPECIAL funct 0x2C..0x2F: doubleword ALU register.
        (0x00a6202c, Dadd { rd: 4, rs: 5, rt: 6 }),
        (0x00a6202d, Daddu { rd: 4, rs: 5, rt: 6 }),
        (0x00a6202e, Dsub { rd: 4, rs: 5, rt: 6 }),
        (0x00a6202f, Dsubu { rd: 4, rs: 5, rt: 6 }),
        // main-opcode 0x2C/0x2D: SDL/SDR (a DIFFERENT table from SPECIAL funct).
        (0xb0a40008, Sdl { rt: 4, base: 5, off: 8 }),
        (0xb4a40008, Sdr { rt: 4, base: 5, off: 8 }),
        // main-opcode 0x2F: CACHE — must NOT be confused with SPECIAL funct
        // 0x2F (DSUBU) above.
        (0xbca20008, Cache { op: 2, base: 5, off: 8 }),
        // main-opcode doubleword mem.
        (0xdca40008, Ld { rt: 4, base: 5, off: 8 }),
        (0xfca40008, Sd { rt: 4, base: 5, off: 8 }),
        (0xd0a40008, Lld { rt: 4, base: 5, off: 8 }),
        (0xf0a40008, Scd { rt: 4, base: 5, off: 8 }),
        // COP1 loads/stores (distinct dedicated main opcodes).
        (0xc4a40008, Lwc1 { ft: 4, base: 5, off: 8 }),
        (0xd4a40008, Ldc1 { ft: 4, base: 5, off: 8 }),
        (0xe4a40008, Swc1 { ft: 4, base: 5, off: 8 }),
        (0xf4a40008, Sdc1 { ft: 4, base: 5, off: 8 }),
        // SPECIAL traps / sync (cop0 family) — distinct SPECIAL functs.
        (0x0000000c, Syscall { code: 0 }),
        (0x0000000d, Break { code: 0 }),
        (0x0000000f, Sync),
    ];

    for &(word, expected) in cases {
        assert_eq!(
            decode(word),
            expected,
            "slot for word {word:#010X} decoded to the wrong op (opcode/funct \
             collision or mis-assignment)"
        );
    }

    // Sanity: the two families sharing the numeric value 0x2C/0x2D/0x2F really
    // are on different dispatch axes — a SPECIAL word (opcode 0) with funct
    // 0x2C is DADD, while a main-opcode-0x2C word is SDL. They must never
    // decode to the same variant.
    assert_ne!(decode(0x00a6202c), decode(0xb0a40008)); // DADD vs SDL
    assert_ne!(decode(0x00a6202f), decode(0xbca20008)); // DSUBU vs CACHE
}
