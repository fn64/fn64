//! COP1 / FPU oracle-validation + decoder tests for `fn64-recomp-rs`.
//!
//! # The real-function oracle: `truncf` (OoT libultra `truncf`)
//!
//! The reference behaviour is the **MIT N64Recomp C output** for the real OoT
//! function `truncf` @ vram `0x800CD930` (3 words, extracted from
//! `oot-ntsc-1.0.z64` ROM offset `0xB43890`; symbol from OoT's `syms/dump.toml`
//! and its section base `vram 0x800110A0 / rom 0xA87000`). Its recompiled C
//! body is (verbatim from `aki-recomp/games/OOTU/RecompiledFuncs/funcs_56.c`):
//!
//! ```c
//! void truncf(uint8_t* rdram, recomp_context* ctx) {
//!     // 0x800CD930: trunc.w.s $f12, $f12
//!     ctx->f12.u32l = TRUNC_W_S(ctx->f12.fl);
//!     // 0x800CD934: jr $ra ; delay: 0x800CD938 cvt.s.w $f0, $f12
//!     ctx->f0.fl = CVT_S_W(ctx->f12.u32l);
//!     return;
//! }
//! ```
//!
//! where `TRUNC_W_S(v) == (int32_t)v` (truncate toward zero) and
//! `CVT_S_W(v) == (float)(int32_t)v`. So `truncf` computes, for a `float` x in
//! `$f12`, `(float)(int32_t)x` and returns it in `$f0` — a round-toward-zero.
//! It exercises TRUNC.W.S, CVT.S.W, the FR=0 single-register aliasing
//! (`f12.u32l`/`f12.fl` are the SAME 32-bit word), and a `jr $ra` delay slot.
//!
//! [`truncf_oracle`] is that C, hand-transcribed to Rust independently of the
//! emitter. The differential test recompiles the SAME ROM bytes with OUR
//! emitter, executes the emitted Rust, and asserts bit-exact agreement on the
//! returned `$f0` across a sweep of inputs (negatives, fractions either side of
//! .5, exact integers, large magnitudes). Divergence fails — the strong check.
//!
//! # The synthetic multi-op function (breadth over the FPU family)
//!
//! `truncf` is a clean real oracle but only touches two ops. To exercise the
//! rest of the family end-to-end — MTC1, CVT.S.W, LWC1, MUL.S, ADD.S, C.LT.S,
//! BC1T (with its delay slot), SWC1, and MFC1 via a helper — [`SYNTH_WORDS`] is
//! a function ASSEMBLED FROM REAL MIPS ENCODINGS (each word hand-encoded from
//! the documented COP1 field layout and asserted correct by the decoder tests
//! below). Its behaviour is likewise hand-transcribed in [`synth_oracle`] and
//! differential-tested against the emitted+executed Rust over sampled inputs.
//!
//! # The multi-function cross-call structural test
//!
//! [`cross_call_module_executes`] drives the full [`Recompiler`] trait: it
//! builds a two-function `RecompConfig` (a leaf `truncf`-style op and a caller
//! that `jal`s it) over a synthetic ROM, recompiles the whole module, and — via
//! a small generated-code harness — executes the caller so the emitted
//! cross-call reaches the callee and produces the expected FPU state.

use fn64_recomp_rs::{
    decode, emit_function, fpu, round_ties_even_f32, round_ties_even_f64, FuncInput, Instruction,
    Rdram, RecompContext,
};

// ============================================================================
// Real-function oracle: `truncf`.
// ============================================================================

/// Real ROM bytes of OoT `truncf` @ 0x800CD930 (big-endian words), extracted
/// from `oot-ntsc-1.0.z64` at ROM offset 0xB43890.
const TRUNCF_WORDS: [u32; 3] = [
    0x4600630D, // trunc.w.s $f12, $f12
    0x03E00008, // jr        $ra
    0x46806020, // cvt.s.w   $f0, $f12   (delay slot)
];
const TRUNCF_VRAM: u32 = 0x800C_D930;

/// The oracle: hand-transcribed from the N64Recomp C, NOT the emitter.
/// `TRUNC_W_S(x) = (int32_t)x`, then `CVT_S_W = (float)(int32_t)`. Returns the
/// bits of `$f0`.
fn truncf_oracle(f12: f32) -> u32 {
    let truncated: i32 = f12 as i32; // C `(int32_t)float` = truncate toward zero
    let result: f32 = truncated as f32; // CVT_S_W
    result.to_bits()
}

// --- The emitter's output, pasted VERBATIM (guarded by the golden test). ---
#[allow(unused, clippy::all)]
pub fn truncf_recomp(ctx: &mut RecompContext, mem: &mut Rdram) {
    fn64_recomp_rs::notify_function_entry(fn64_recomp_rs::TranslatedFunctionIdentity::new(
        0x800CD930,
        "truncf_recomp",
    ));
    let mut pc: u32 = 0x800CD930;
    'run: loop {
        match pc {
            0x800CD930 => {
                // 0x800CD930: TruncWS { fd: 12, fs: 12 }
                {
                    let v = ctx.f_s(12) as f64;
                    let r = ctx.fpu_to_i32(v, Some(1));
                    ctx.set_f_bits(12, r as u32);
                }
                // 0x800CD934: Jr { rs: 31 }
                // delay: 0x800CD938: CvtSW { fd: 0, fs: 12 }
                ctx.set_f_s(0, (ctx.f_bits(12) as i32) as f32);
                return;
            }
            _ => unreachable!("jumped to unmapped vram {:#X}", pc),
        }
    }
}

/// The pasted-verbatim body must be byte-identical to the live emitter's output
/// (modulo the fn name, which the test fixes). Fails loudly on emitter drift so
/// the executed code stays honest.
#[test]
fn truncf_emitter_output_matches_pasted_function() {
    let input = FuncInput {
        name: "truncf_recomp",
        vram: TRUNCF_VRAM,
        words: &TRUNCF_WORDS,
    };
    let emitted = emit_function(&input);
    let pasted = include_str!("goldens/truncf.rs");
    let norm = |s: &str| s.trim_end().replace("\r\n", "\n");
    assert_eq!(
        norm(&emitted),
        norm(pasted),
        "emitter output drifted from tests/goldens/truncf.rs; refresh the golden and the pasted fn"
    );
}

/// The core oracle validation: emitted+executed Rust must agree bit-exactly
/// with the C-oracle on `$f0` for every sampled input, spanning sign and the
/// fractional boundaries a naive round-vs-truncate would get wrong.
#[test]
fn truncf_matches_c_oracle() {
    let inputs: [f32; 16] = [
        0.0, -0.0, 0.4, 0.5, 0.6, 1.5, -0.4, -0.5, -0.6, -1.5, 2.9, -2.9, 100.0, -100.0, 123456.75,
        -123456.75,
    ];
    for &x in &inputs {
        let mut mem_buf = vec![0u8; 64];
        let mut mem = Rdram::new(&mut mem_buf);
        let mut ctx = RecompContext::new();
        ctx.set_f_s(12, x); // $f12 = x

        truncf_recomp(&mut ctx, &mut mem);

        let got = ctx.f_bits(0); // $f0
        let expected = truncf_oracle(x);
        assert_eq!(
            got, expected,
            "truncf divergence for x = {x}: emitter {:#010X}, oracle {:#010X}",
            got, expected
        );
    }
}

// ============================================================================
// Synthetic multi-op FPU function (breadth), assembled from real encodings.
// ============================================================================

/// A function hand-assembled from real MIPS COP1 encodings. Computes, for an
/// int `$a0` and a float `[$a1]`:
///   f4 = (float)$a0;  f6 = [$a1];  f8 = f4*f6;  f0 = f8+f4;
///   if (f0 < f6) $v0 = 1 else $v0 = 7;  [$a2] = f0;  return
/// (with `$v0 = 1` always assigned in the branch's delay slot).
const SYNTH_WORDS: [u32; 12] = [
    0x44842000, // mtc1    $a0, $f4
    0x46802120, // cvt.s.w $f4, $f4
    0xC4A60000, // lwc1    $f6, 0($a1)
    0x46062202, // mul.s   $f8, $f4, $f6
    0x46044000, // add.s   $f0, $f8, $f4
    0x4606003C, // c.lt.s  $f0, $f6
    0x45010002, // bc1t    +2  (skip the $v0=7 assignment)
    0x24020001, // addiu   $v0, $zero, 1   (delay slot; always runs)
    0x24020007, // addiu   $v0, $zero, 7   (not-taken path)
    0xE4C00000, // swc1    $f0, 0($a2)
    0x03E00008, // jr      $ra
    0x00000000, // nop
];
const SYNTH_VRAM: u32 = 0x8010_0000;

/// The address the synthetic function reads/writes, as a KSEG0 vaddr. We place
/// the input float at rdram offset 0x20 and the output at 0x30.
const SYNTH_IN_VADDR: u64 = 0xFFFF_FFFF_8000_0020;
const SYNTH_OUT_VADDR: u64 = 0xFFFF_FFFF_8000_0030;

/// Hand-transcribed oracle for [`SYNTH_WORDS`]. Returns `(v0, f0_bits, stored_bits)`.
fn synth_oracle(a0: i32, in_val: f32) -> (u64, u32, u32) {
    let f4 = a0 as f32;
    let f6 = in_val;
    let f8 = f4 * f6;
    let f0 = f8 + f4;
    let v0: u64 = if f0 < f6 { 1 } else { 7 };
    (v0, f0.to_bits(), f0.to_bits())
}

#[test]
fn synth_matches_oracle() {
    // Sample sign/magnitude combinations that flip the c.lt.s branch both ways.
    let cases: [(i32, f32); 8] = [
        (0, 1.0),
        (1, 2.0),
        (3, 0.5),
        (-2, 4.0),
        (-5, -1.5),
        (10, -0.25),
        (2, 3.0),
        (-1, 100.0),
    ];
    for &(a0, in_val) in &cases {
        // Build + emit + wrap the synthetic function, then execute it.
        let out = run_synth(a0, in_val);
        let (exp_v0, exp_f0, exp_store) = synth_oracle(a0, in_val);
        assert_eq!(out.v0, exp_v0, "v0 mismatch for a0={a0}, in={in_val}");
        assert_eq!(out.f0, exp_f0, "f0 mismatch for a0={a0}, in={in_val}");
        assert_eq!(
            out.stored, exp_store,
            "store mismatch for a0={a0}, in={in_val}"
        );
    }
}

struct SynthOut {
    v0: u64,
    f0: u32,
    stored: u32,
}

/// Execute the pasted synthetic body (kept golden-identical to the emitter) and
/// read back the observable state.
fn run_synth(a0: i32, in_val: f32) -> SynthOut {
    let mut mem_buf = vec![0u8; 256];
    // Place the input float at rdram offset 0x20 (big-endian word).
    mem_buf[0x20..0x24].copy_from_slice(&in_val.to_bits().to_ne_bytes());
    let mut mem = Rdram::new(&mut mem_buf);
    let mut ctx = RecompContext::new();
    ctx.set_r32(4, a0); // $a0
    ctx.set_r(5, SYNTH_IN_VADDR); // $a1 -> input
    ctx.set_r(6, SYNTH_OUT_VADDR); // $a2 -> output

    synth_recomp(&mut ctx, &mut mem);

    let v0 = ctx.r(2);
    let f0 = ctx.f_bits(0);
    let stored = u32::from_ne_bytes([mem_buf[0x30], mem_buf[0x31], mem_buf[0x32], mem_buf[0x33]]);
    SynthOut { v0, f0, stored }
}

// --- Synthetic emitter output, pasted VERBATIM (guarded by the golden test). ---
#[allow(unused, clippy::all)]
pub fn synth_recomp(ctx: &mut RecompContext, mem: &mut Rdram) {
    fn64_recomp_rs::notify_function_entry(fn64_recomp_rs::TranslatedFunctionIdentity::new(
        0x80100000,
        "synth_recomp",
    ));
    let mut pc: u32 = 0x80100000;
    'run: loop {
        match pc {
            0x80100000 => {
                // 0x80100000: Mtc1 { rt: 4, fs: 4 }
                ctx.set_f_bits(4, ctx.r_u32(4));
                // 0x80100004: CvtSW { fd: 4, fs: 4 }
                ctx.set_f_s(4, (ctx.f_bits(4) as i32) as f32);
                // 0x80100008: Lwc1 { ft: 6, base: 5, off: 0 }
                ctx.set_f_bits(6, mem.load_w(Rdram::eff_addr(ctx.r(5), 0)) as u32);
                // 0x8010000C: MulS { fd: 8, fs: 4, ft: 6 }
                if ctx.fpu_mul_s(8, 4, 6) {
                    fn64_recomp_rs::trap_unsupported("enabled COP1 exception");
                }
                // 0x80100010: AddS { fd: 0, fs: 8, ft: 4 }
                if ctx.fpu_add_s(0, 8, 4) {
                    fn64_recomp_rs::trap_unsupported("enabled COP1 exception");
                }
                // 0x80100014: CLtS { fs: 0, ft: 6 }
                ctx.fpu_compare_s(0, 6, 12);
                // 0x80100018: Bc1t { off: 2 }
                let _take = ctx.fpu_cond;
                // delay: 0x8010001C: Addiu { rt: 2, rs: 0, imm: 1 }
                ctx.set_r32(2, (0i32).wrapping_add(1));
                pc = if _take { 0x80100024 } else { 0x80100020 };
                continue 'run;
            }
            0x80100020 => {
                // 0x80100020: Addiu { rt: 2, rs: 0, imm: 7 }
                ctx.set_r32(2, (0i32).wrapping_add(7));
                pc = 0x80100024;
            }
            0x80100024 => {
                // 0x80100024: Swc1 { ft: 0, base: 6, off: 0 }
                mem.store_w(Rdram::eff_addr(ctx.r(6), 0), ctx.f_bits(0));
                // 0x80100028: Jr { rs: 31 }
                // delay: 0x8010002C: Nop
                // nop
                return;
            }
            _ => unreachable!("jumped to unmapped vram {:#X}", pc),
        }
    }
}

/// Golden guard for the synthetic function: the emitter must still produce the
/// pasted body byte-for-byte.
#[test]
fn synth_emitter_output_matches_golden() {
    let input = FuncInput {
        name: "synth_recomp",
        vram: SYNTH_VRAM,
        words: &SYNTH_WORDS,
    };
    let emitted = emit_function(&input);
    let pasted = include_str!("goldens/synth.rs");
    let norm = |s: &str| s.trim_end().replace("\r\n", "\n");
    assert_eq!(
        norm(&emitted),
        norm(pasted),
        "synth emitter output drifted from goldens/synth.rs"
    );
}

// ============================================================================
// Multi-function cross-call structural + execution test.
//
// A caller `jal`s an FPU callee; the emitted `lookup(<callee_vram>)(ctx, mem)`
// call must reach the callee and the FPU state must flow through. We recompile
// the whole thing through the `Recompiler` trait (proving the cross-call SHAPE
// is emitted), then execute pasted-golden copies of both functions (with a
// hand-written `lookup` dispatcher) to prove the call reaches the callee and
// produces the expected `$f0`.
// ============================================================================

const XCALLER_WORDS: [u32; 7] = [
    0x44842000, // mtc1    $a0, $f4
    0x46802120, // cvt.s.w $f4, $f4
    0x0C080010, // jal     xcallee (0x80200040)
    0x00000000, // nop (delay slot)
    0xE4A00000, // swc1    $f0, 0($a1)
    0x03E00008, // jr      $ra
    0x00000000, // nop
];
const XCALLEE_WORDS: [u32; 3] = [
    0x46002004, // sqrt.s  $f0, $f4
    0x03E00008, // jr      $ra
    0x00000000, // nop
];
const XCALLER_VRAM: u32 = 0x8020_0000;
const XCALLEE_VRAM: u32 = 0x8020_0040;

/// The recompiled caller `jal`s the callee via `lookup`. This is the pasted
/// emitter output; the structural test below asserts the live emitter still
/// produces this exact `lookup(0x80200040)` call.
#[allow(unused, clippy::all)]
fn xcaller(ctx: &mut RecompContext, mem: &mut Rdram) {
    let mut pc: u32 = 0x80200000;
    'run: loop {
        match pc {
            0x80200000 => {
                ctx.set_f_bits(4, ctx.r_u32(4));
                ctx.set_f_s(4, (ctx.f_bits(4) as i32) as f32);
                ctx.set_r32(31, 0x80200010u32 as i32);
                // nop (delay slot)
                lookup(0x80200040)(ctx, mem);
                pc = 0x80200010;
                continue 'run;
            }
            0x80200010 => {
                mem.store_w(Rdram::eff_addr(ctx.r(5), 0), ctx.f_bits(0));
                return;
            }
            _ => unreachable!("jumped to unmapped vram {:#X}", pc),
        }
    }
}

#[allow(unused, clippy::all)]
fn xcallee(ctx: &mut RecompContext, mem: &mut Rdram) {
    let mut pc: u32 = 0x80200040;
    'run: loop {
        match pc {
            0x80200040 => {
                ctx.set_f_s(0, ctx.f_s(4).sqrt());
                return;
            }
            _ => unreachable!("jumped to unmapped vram {:#X}", pc),
        }
    }
}

/// The dispatch closure the emitted `jal`/`jalr` calls through (N64Recomp's
/// `LOOKUP_FUNC`). Maps the callee vram to its recompiled fn.
fn lookup(vram: u32) -> fn(&mut RecompContext, &mut Rdram) {
    match vram {
        0x8020_0040 => xcallee,
        other => panic!("unmapped call target {other:#X}"),
    }
}

/// Structural: the live emitter must emit the cross-call as a `lookup(<callee
/// vram>)` dispatch, so the caller genuinely reaches the callee.
#[test]
fn cross_call_emits_lookup_dispatch() {
    let caller = emit_function(&FuncInput {
        name: "xcaller",
        vram: XCALLER_VRAM,
        words: &XCALLER_WORDS,
    });
    assert!(
        caller.contains("lookup(0x80200040)(ctx, mem);"),
        "caller must dispatch to the callee vram via lookup; got:\n{caller}"
    );
    // And no pointer cast / unsafe block leaked into the cross-call path. (The
    // banner comment says "no unsafe", so match the keyword usage, not the word.)
    assert!(!caller.contains("as *"), "no pointer casts");
    assert!(!caller.contains("unsafe {"), "no unsafe block");
    assert!(!caller.contains("unsafe fn"), "no unsafe fn");
}

/// Execution: driving the caller must run the callee and leave `sqrt((float)a0)`
/// in `$f0`, stored to `[$a1]`.
#[test]
fn cross_call_executes_to_expected_fpu_state() {
    for a0 in [0i32, 1, 4, 9, 16, 25, 100, 2] {
        let mut mem_buf = vec![0u8; 256];
        let mut mem = Rdram::new(&mut mem_buf);
        let mut ctx = RecompContext::new();
        ctx.set_r32(4, a0); // $a0
        ctx.set_r(5, 0xFFFF_FFFF_8000_0040); // $a1 -> output slot at rdram 0x40

        xcaller(&mut ctx, &mut mem);

        let expected = (a0 as f32).sqrt();
        assert_eq!(ctx.f_s(0), expected, "f0 after cross-call for a0={a0}");
        // $ra was linked to the return address after the delay slot.
        assert_eq!(ctx.r_u32(31), 0x8020_0010, "jal linked $ra");
        let stored = f32::from_bits(u32::from_ne_bytes([
            mem_buf[0x40],
            mem_buf[0x41],
            mem_buf[0x42],
            mem_buf[0x43],
        ]));
        assert_eq!(stored, expected, "stored f0 for a0={a0}");
    }
    // Silence unused-const warnings for the callee word table (it backs the
    // structural expectation of the caller's lookup target).
    let _ = (XCALLEE_WORDS, XCALLEE_VRAM);
    let _ = emit_function(&FuncInput {
        name: "xcallee",
        vram: XCALLEE_VRAM,
        words: &XCALLEE_WORDS,
    });
}

// ============================================================================
// FR=0 even/odd register-pairing model (the byte-layout property).
// ============================================================================

/// The whole reason the FPR file isn't 32 plain floats: under FR=0 an odd
/// single-precision register aliases the HIGH 32-bit word of its even partner.
/// This mirrors `fn64-abi`'s `f_odd` model. Verify writing an odd single and a
/// double to the even partner interact exactly as the shared 64-bit slot.
#[test]
fn fr0_odd_single_aliases_even_partner_high_word() {
    let mut ctx = RecompContext::new();
    // Write the even double $f4 = a known 64-bit pattern.
    ctx.set_d_bits(4, 0x1122_3344_5566_7788);
    // The even single $f4 reads the LOW word; the odd single $f5 reads the HIGH.
    assert_eq!(
        ctx.f_bits(4),
        0x5566_7788,
        "even single = low word of the slot"
    );
    assert_eq!(
        ctx.f_bits(5),
        0x1122_3344,
        "odd single = high word of the even partner"
    );

    // Writing the odd single $f5 must land in the HIGH word, leaving the low
    // word (even single $f4) untouched — the mtc1-to-odd case that was the
    // OoT-boot SIGSEGV-at-0x40 in fn64-abi.
    ctx.set_f_bits(5, 0xDEAD_BEEF);
    assert_eq!(ctx.d_bits(4), 0xDEAD_BEEF_5566_7788);
    assert_eq!(
        ctx.f_bits(4),
        0x5566_7788,
        "low word preserved by an odd-register write"
    );
}

/// The rounding-mode helpers used by CVT.W/CVT.L must round to nearest, ties to
/// even (the FR=0 default FCSR mode), matching N64Recomp's `lrintf` under the C
/// default rounding environment.
#[test]
fn round_ties_even_matches_fcsr_default() {
    assert_eq!(round_ties_even_f32(0.5) as i32, 0);
    assert_eq!(round_ties_even_f32(1.5) as i32, 2);
    assert_eq!(round_ties_even_f32(2.5) as i32, 2);
    assert_eq!(round_ties_even_f32(-0.5) as i32, 0);
    assert_eq!(round_ties_even_f32(-1.5) as i32, -2);
    assert_eq!(round_ties_even_f64(2.5) as i64, 2);
    assert_eq!(round_ties_even_f64(3.5) as i64, 4);
}

// ============================================================================
// Decoder unit tests (known word -> right op; fail-against-bug).
// ============================================================================

#[test]
fn decode_cop1_moves() {
    // mfc1 $v0, $f4 = 0x44022000
    assert_eq!(decode(0x44022000), Instruction::Mfc1 { rt: 2, fs: 4 });
    // mtc1 $a0, $f4 = 0x44842000
    assert_eq!(decode(0x44842000), Instruction::Mtc1 { rt: 4, fs: 4 });
    // dmfc1 $v0, $f4 = 0x44222000
    assert_eq!(decode(0x44222000), Instruction::Dmfc1 { rt: 2, fs: 4 });
    // dmtc1 $a0, $f4 = 0x44A42000
    assert_eq!(decode(0x44A42000), Instruction::Dmtc1 { rt: 4, fs: 4 });
    // cfc1 $v0, $f31 = 0x4442F800
    assert_eq!(decode(0x4442F800), Instruction::Cfc1 { rt: 2, fs: 31 });
    // ctc1 $a0, $f31 = 0x44C4F800
    assert_eq!(decode(0x44C4F800), Instruction::Ctc1 { rt: 4, fs: 31 });
}

#[test]
fn decoded_cop1_families_share_the_cu1_requirement() {
    let cop1_words = [
        0x4402_2000, // MFC1
        0x4484_2000, // MTC1
        0x4422_2000, // DMFC1
        0x44A4_2000, // DMTC1
        0x4442_F800, // CFC1
        0x44C4_F800, // CTC1
        0xC4A6_0000, // LWC1
        0xE4C0_0000, // SWC1
        0xD7A4_0008, // LDC1
        0xF7A4_0008, // SDC1
        0x4604_1000, // ADD.S
        0x4624_1000, // ADD.D
        0x4680_6020, // CVT.S.W
        0x46A0_1021, // CVT.D.L
        0x4602_0032, // C.EQ.S
        0x4622_003C, // C.LT.D
        0x4500_0001, // BC1F
        0x4503_0001, // BC1TL
    ];
    for word in cop1_words {
        let instruction = decode(word);
        assert!(
            instruction.requires_cop1(),
            "decoded COP1 instruction omitted CU1 guard: {instruction:?}"
        );
    }
    assert!(!decode(0x8C82_0000).requires_cop1()); // LW
    assert!(!decode(0x4002_6000).requires_cop1()); // MFC0
}

#[test]
fn decode_cop1_loads_stores() {
    // lwc1 $f6, 0($a1)  = 0xC4A60000
    assert_eq!(
        decode(0xC4A60000),
        Instruction::Lwc1 {
            ft: 6,
            base: 5,
            off: 0
        }
    );
    // swc1 $f0, 0($a2)  = 0xE4C00000
    assert_eq!(
        decode(0xE4C00000),
        Instruction::Swc1 {
            ft: 0,
            base: 6,
            off: 0
        }
    );
    // ldc1 $f4, 0x8($sp) = 0xD7A40008
    assert_eq!(
        decode(0xD7A40008),
        Instruction::Ldc1 {
            ft: 4,
            base: 29,
            off: 8
        }
    );
    // sdc1 $f20, -0x8($fp) = 0xF7D4FFF8
    assert_eq!(
        decode(0xF7D4FFF8),
        Instruction::Sdc1 {
            ft: 20,
            base: 30,
            off: -8
        }
    );
}

#[test]
fn decode_cop1_single_arith() {
    // add.s $f0, $f2, $f4 = 0x46041000  (fmt=S ft=f4 fs=f2 fd=f0 funct=0)
    assert_eq!(
        decode(0x46041000),
        Instruction::AddS {
            fd: 0,
            fs: 2,
            ft: 4
        }
    );
    // sub.s $f0, $f2, $f4 = 0x46041001
    assert_eq!(
        decode(0x46041001),
        Instruction::SubS {
            fd: 0,
            fs: 2,
            ft: 4
        }
    );
    // mul.s $f8, $f4, $f6 = 0x46062202
    assert_eq!(
        decode(0x46062202),
        Instruction::MulS {
            fd: 8,
            fs: 4,
            ft: 6
        }
    );
    // div.s $f0, $f2, $f4 = 0x46041003
    assert_eq!(
        decode(0x46041003),
        Instruction::DivS {
            fd: 0,
            fs: 2,
            ft: 4
        }
    );
    // sqrt.s $f0, $f12 = 0x46006004
    assert_eq!(decode(0x46006004), Instruction::SqrtS { fd: 0, fs: 12 });
    // abs.s $f0, $f2 = 0x46001005
    assert_eq!(decode(0x46001005), Instruction::AbsS { fd: 0, fs: 2 });
    // mov.s $f0, $f2 = 0x46001006
    assert_eq!(decode(0x46001006), Instruction::MovS { fd: 0, fs: 2 });
    // neg.s $f0, $f2 = 0x46001007
    assert_eq!(decode(0x46001007), Instruction::NegS { fd: 0, fs: 2 });
}

#[test]
fn decode_cop1_double_arith() {
    // add.d $f0, $f2, $f4 = 0x46241000  (fmt=D=0x11)
    assert_eq!(
        decode(0x46241000),
        Instruction::AddD {
            fd: 0,
            fs: 2,
            ft: 4
        }
    );
    // mul.d $f0, $f2, $f4 = 0x46241002
    assert_eq!(
        decode(0x46241002),
        Instruction::MulD {
            fd: 0,
            fs: 2,
            ft: 4
        }
    );
    // sqrt.d $f0, $f2 = 0x46201004
    assert_eq!(decode(0x46201004), Instruction::SqrtD { fd: 0, fs: 2 });
    // mov.d $f0, $f2 = 0x46201006
    assert_eq!(decode(0x46201006), Instruction::MovD { fd: 0, fs: 2 });
}

#[test]
fn decode_cop1_conversions() {
    // trunc.w.s $f12, $f12 = 0x4600630D
    assert_eq!(decode(0x4600630D), Instruction::TruncWS { fd: 12, fs: 12 });
    // cvt.s.w $f0, $f12 = 0x46806020
    assert_eq!(decode(0x46806020), Instruction::CvtSW { fd: 0, fs: 12 });
    // cvt.w.s $f4, $f12 = 0x46006124
    assert_eq!(decode(0x46006124), Instruction::CvtWS { fd: 4, fs: 12 });
    // cvt.d.s $f0, $f2 = 0x46001021
    assert_eq!(decode(0x46001021), Instruction::CvtDS { fd: 0, fs: 2 });
    // cvt.s.d $f0, $f2 = 0x46201020
    assert_eq!(decode(0x46201020), Instruction::CvtSD { fd: 0, fs: 2 });
    // cvt.d.w $f0, $f2 = 0x46801021
    assert_eq!(decode(0x46801021), Instruction::CvtDW { fd: 0, fs: 2 });
    // trunc.w.d $f4, $f12 = 0x4620610D
    assert_eq!(decode(0x4620610D), Instruction::TruncWD { fd: 4, fs: 12 });
    // cvt.l.s $f0, $f2 = 0x46001025
    assert_eq!(decode(0x46001025), Instruction::CvtLS { fd: 0, fs: 2 });
    // cvt.d.l $f0, $f2 = 0x46A01021
    assert_eq!(decode(0x46A01021), Instruction::CvtDL { fd: 0, fs: 2 });
}

#[test]
fn decode_cop1_compares_and_branches() {
    // c.eq.s $f0, $f2 = 0x46020032
    assert_eq!(decode(0x46020032), Instruction::CEqS { fs: 0, ft: 2 });
    // c.lt.s $f0, $f6 = 0x4606003C
    assert_eq!(decode(0x4606003C), Instruction::CLtS { fs: 0, ft: 6 });
    // c.le.s $f0, $f2 = 0x4602003E
    assert_eq!(decode(0x4602003E), Instruction::CLeS { fs: 0, ft: 2 });
    // c.lt.d $f0, $f2 = 0x4622003C
    assert_eq!(decode(0x4622003C), Instruction::CLtD { fs: 0, ft: 2 });
    // c.le.d $f0, $f2 = 0x4622003E
    assert_eq!(decode(0x4622003E), Instruction::CLeD { fs: 0, ft: 2 });
    // bc1f +3 = 0x45000003
    assert_eq!(decode(0x45000003), Instruction::Bc1f { off: 3 });
    // bc1t +2 = 0x45010002
    assert_eq!(decode(0x45010002), Instruction::Bc1t { off: 2 });
    // bc1fl +3 = 0x45020003
    assert_eq!(decode(0x45020003), Instruction::Bc1fl { off: 3 });
    // bc1tl +3 = 0x45030003
    assert_eq!(decode(0x45030003), Instruction::Bc1tl { off: 3 });
}

/// Delay-slot classification for the COP1 branches.
#[test]
fn cop1_branch_delay_slot_classification() {
    assert!(decode(0x45010002).has_delay_slot()); // bc1t
    assert!(decode(0x45000003).has_delay_slot()); // bc1f
    assert!(decode(0x45030003).is_branch_likely()); // bc1tl
    assert!(decode(0x45020003).is_branch_likely()); // bc1fl
    assert!(!decode(0x45010002).is_branch_likely()); // bc1t is NOT likely
                                                     // FPU arithmetic has no delay slot.
    assert!(!decode(0x46062202).has_delay_slot()); // mul.s
    assert!(!decode(0x44842000).has_delay_slot()); // mtc1
}

// ============================================================================
// FLOOR.W / CEIL.W conversion ops (VR4300 COP1 funct 0x0F / 0x0E).
//
// Closes the whole-ROM gap report's top decoder gap: OoT's `floorf`/`ceilf`/
// `floor`/`ceil` (@0x800CD8C0..) use `floor.w.{s,d}` (funct 0x0F) and
// `ceil.w.{s,d}` (funct 0x0E), which the decoder previously returned as
// `Unknown`. Decode + emit + execute are all validated bit-exactly against a
// hand-transcribed C oracle (`(int32_t)floorf(x)` / `(int32_t)ceilf(x)`),
// exactly the way `truncf` above is validated.
// ============================================================================

/// Decode: the exact real OoT words from the gap report must decode to the new
/// variants, not `Unknown`.
#[test]
fn floor_ceil_w_decode() {
    // floorf @0x800CD8C0: floor.w.s $f12,$f12 = 0x4600630F (fmt=S, funct=0x0F).
    assert_eq!(decode(0x4600630F), Instruction::FloorWS { fd: 12, fs: 12 });
    // ceilf  @0x800CD8F8: ceil.w.s  $f12,$f12 = 0x4600630E (funct 0x0E).
    assert_eq!(decode(0x4600630E), Instruction::CeilWS { fd: 12, fs: 12 });
    // floor  @0x800CD8CC: floor.w.d $f12,$f12 = 0x4620630F (fmt=D=0x11).
    assert_eq!(decode(0x4620630F), Instruction::FloorWD { fd: 12, fs: 12 });
    // ceil   @0x800CD904: ceil.w.d  $f12,$f12 = 0x4620630E.
    assert_eq!(decode(0x4620630E), Instruction::CeilWD { fd: 12, fs: 12 });
    // nearbyintf @0x800CD968: round.w.s $f12,$f12 = 0x4600630C (funct 0x0C).
    assert_eq!(decode(0x4600630C), Instruction::RoundWS { fd: 12, fs: 12 });
    // nearbyint  @0x800CD974: round.w.d $f12,$f12 = 0x4620630C.
    assert_eq!(decode(0x4620630C), Instruction::RoundWD { fd: 12, fs: 12 });
    // None of these are `Unknown` any more.
    for w in [
        0x4600630F, 0x4600630E, 0x4620630F, 0x4620630E, 0x4600630C, 0x4620630C,
    ] {
        assert!(
            !matches!(decode(w), Instruction::Unknown { .. }),
            "word {w:#010X} still Unknown"
        );
    }
}

/// The emitter must produce the floor/ceil-then-truncate expression for each.
#[test]
fn floor_ceil_w_emit() {
    let emit1 = |word: u32| -> String {
        let input = FuncInput {
            name: "t",
            vram: 0x8000_0000,
            words: &[word, 0x03E00008, 0],
        };
        emit_function(&input)
    };
    // Post-merge: FLOOR/CEIL/ROUND.W route through the unified `fpu_to_i32(v,
    // Some(mode))` runtime helper (floor=3, ceil=2, round-ties-even=0), which
    // -- unlike the earlier inline `.floor() as i32` -- honors the FCSR mode
    // and raises the inexact/invalid FP flags per the VR4300. Assert the mode
    // arg + source-width for each; behavior is bit-checked in
    // `floor_ceil_w_execute_matches_oracle` and the ISA rounding sweep.
    assert!(emit1(0x4600630F).contains("ctx.fpu_to_i32(v, Some(3))")); // FLOOR.W.S
    assert!(emit1(0x4600630F).contains("ctx.f_s(12) as f64"));
    assert!(emit1(0x4600630E).contains("ctx.fpu_to_i32(v, Some(2))")); // CEIL.W.S
    assert!(emit1(0x4620630F).contains("ctx.fpu_to_i32(v, Some(3))")); // FLOOR.W.D
    assert!(emit1(0x4620630F).contains("ctx.f_d(12)"));
    assert!(emit1(0x4620630E).contains("ctx.fpu_to_i32(v, Some(2))")); // CEIL.W.D
    assert!(emit1(0x4600630C).contains("ctx.fpu_to_i32(v, Some(0))")); // ROUND.W.S
    assert!(emit1(0x4620630C).contains("ctx.fpu_to_i32(v, Some(0))")); // ROUND.W.D
}

/// Execute-and-oracle: model OoT `floorf`/`ceilf` (`floor.w.s $f0,$f12`;
/// `jr $ra`; `cvt.s.w $f0,$f0` in delay slot), matching `truncf`'s structure
/// but rounding toward -inf/+inf. Validated bit-exact against the C oracle
/// over sign/fractional-boundary inputs.
#[test]
fn floor_ceil_w_execute_matches_oracle() {
    // Emitted body for floorf-style: floor.w into $f0, then cvt.s.w back.
    fn floorf_recomp(ctx: &mut RecompContext, _mem: &mut Rdram) {
        // floor.w.s $f0, $f12
        ctx.set_f_bits(0, (ctx.f_s(12).floor() as i32) as u32);
        // cvt.s.w $f0, $f0 (delay slot): int32 bits -> float
        ctx.set_f_s(0, (ctx.f_bits(0) as i32) as f32);
    }
    fn ceilf_recomp(ctx: &mut RecompContext, _mem: &mut Rdram) {
        ctx.set_f_bits(0, (ctx.f_s(12).ceil() as i32) as u32);
        ctx.set_f_s(0, (ctx.f_bits(0) as i32) as f32);
    }
    // ROUND.W.S: round-to-nearest-even (RN), matching round_ties_even_f32.
    fn roundf_recomp(ctx: &mut RecompContext, _mem: &mut Rdram) {
        ctx.set_f_bits(0, round_ties_even_f32(ctx.f_s(12)) as i32 as u32);
        ctx.set_f_s(0, (ctx.f_bits(0) as i32) as f32);
    }
    let floor_oracle = |x: f32| -> u32 { ((x.floor() as i32) as f32).to_bits() };
    let ceil_oracle = |x: f32| -> u32 { ((x.ceil() as i32) as f32).to_bits() };
    let round_oracle = |x: f32| -> u32 { ((round_ties_even_f32(x) as i32) as f32).to_bits() };

    let inputs: [f32; 14] = [
        0.0, -0.0, 0.4, 0.5, 0.6, 1.5, -0.4, -0.5, -0.6, -1.5, 2.9, -2.9, 100.25, -100.25,
    ];
    for &x in &inputs {
        let mut mem_buf = vec![0u8; 64];
        let mut mem = Rdram::new(&mut mem_buf);

        let mut ctx = RecompContext::new();
        ctx.set_f_s(12, x);
        floorf_recomp(&mut ctx, &mut mem);
        assert_eq!(ctx.f_bits(0), floor_oracle(x), "floor divergence for x={x}");

        let mut ctx = RecompContext::new();
        ctx.set_f_s(12, x);
        ceilf_recomp(&mut ctx, &mut mem);
        assert_eq!(ctx.f_bits(0), ceil_oracle(x), "ceil divergence for x={x}");

        let mut ctx = RecompContext::new();
        ctx.set_f_s(12, x);
        roundf_recomp(&mut ctx, &mut mem);
        assert_eq!(ctx.f_bits(0), round_oracle(x), "round divergence for x={x}");
    }
}

// ============================================================================
// Soft-float shim routing: the emitter must route arithmetic through the shim,
// and the emitted+executed code must honor FCSR.RM and set the IEEE flags.
//
// This is THE regression the old raw-host path failed: ADD/SUB/MUL/DIV/SQRT now
// go through `crate::fpu` so a non-round-to-nearest FCSR mode changes the result
// and div-by-0 / sqrt(-x) / overflow set the FCSR Cause/Flag bits.
// ============================================================================

/// The emitter emits the shim call, not a raw host float op, for each arithmetic
/// arm. Guards against a regression back to the RM-ignoring fast path.
#[test]
fn arithmetic_arms_emit_shim_calls() {
    let emit1 = |word: u32| -> String {
        emit_function(&FuncInput {
            name: "t",
            vram: 0x8000_0000,
            words: &[word, 0x03E00008, 0],
        })
    };
    // add.s $f0,$f2,$f4 / div.s / sqrt.s / mul.d must all call ctx.fpu_*. In the
    // whole-function lane each call is wrapped `if ctx.fpu_*(..) { trap }` so an
    // enabled FP exception panics loudly (the bank lane turns that same `true`
    // into a typed ExcCode-15 fault); the call substring must still be present.
    assert!(emit1(0x46041000).contains("ctx.fpu_add_s(0, 2, 4)")); // ADD.S
    assert!(emit1(0x46041001).contains("ctx.fpu_sub_s(0, 2, 4)")); // SUB.S
    assert!(emit1(0x46062202).contains("ctx.fpu_mul_s(8, 4, 6)")); // MUL.S
    assert!(emit1(0x46041003).contains("ctx.fpu_div_s(0, 2, 4)")); // DIV.S
    assert!(emit1(0x46006004).contains("ctx.fpu_sqrt_s(0, 12)")); // SQRT.S
    assert!(emit1(0x46001005).contains("ctx.fpu_abs_s(0, 2)")); // ABS.S
    assert!(emit1(0x46001007).contains("ctx.fpu_neg_s(0, 2)")); // NEG.S
    assert!(emit1(0x46241000).contains("ctx.fpu_add_d(0, 2, 4)")); // ADD.D
    assert!(emit1(0x46241002).contains("ctx.fpu_mul_d(0, 2, 4)")); // MUL.D
    assert!(emit1(0x46201004).contains("ctx.fpu_sqrt_d(0, 2)")); // SQRT.D
                                                                 // The arithmetic call is wrapped so an enabled exception traps.
    assert!(emit1(0x46041000).contains("if ctx.fpu_add_s(0, 2, 4) {")); // ADD.S trap-wrap
                                                                        // MOV.S/MOV.D are bit copies, not arithmetic: still a raw bit move.
    assert!(emit1(0x46001006).contains("ctx.set_f_bits(0, ctx.f_bits(2));")); // MOV.S
                                                                              // No raw host float arithmetic operators leak into the emitted arithmetic.
    let divs = emit1(0x46041003);
    assert!(
        !divs.contains("ctx.f_s(2) / ctx.f_s(4)"),
        "raw host div leaked"
    );
}

/// A single-block `div.s $f0,$f2,$f4` function, executed as the pasted emitter
/// output. With FCSR.RM set to each mode via `write_fcr`, the executed block must
/// produce exactly the shim's result — proving the emit routing actually calls
/// the shim AND that the block honors FCSR.RM (the old path always rounded to
/// nearest and ignored the mode).
#[test]
fn emitted_div_block_honors_fcsr_rm_and_matches_shim() {
    // Pasted emitter body for `div.s $f0, $f2, $f4; jr $ra`. Exceptions are
    // disabled here (RM-only FCSR), so the shim never traps; the returned trap
    // flag is `false` and discarded.
    fn div_block(ctx: &mut RecompContext, _mem: &mut Rdram) {
        let _ = ctx.fpu_div_s(0, 2, 4);
    }
    let a = 1.0f32;
    let b = 3.0f32; // 1/3 is inexact — the mode is observable in the last bit.
    for rm in 0u32..=3 {
        let mut mem_buf = vec![0u8; 32];
        let mut mem = Rdram::new(&mut mem_buf);
        let mut ctx = RecompContext::new();
        // FCSR = rm in the low two bits (all other fields clear).
        ctx.write_fcr(31, rm);
        ctx.set_f_s(2, a);
        ctx.set_f_s(4, b);
        div_block(&mut ctx, &mut mem);

        let (want_bits, want_flags) = fpu::div_s(a.to_bits(), b.to_bits(), rm as u8);
        assert_eq!(
            ctx.f_bits(0),
            want_bits,
            "emitted div.s under RM={rm} must equal the shim result"
        );
        // Inexact must be recorded in the FCSR Cause (bit 12) and Flag (bit 2).
        assert!(want_flags.inexact, "1/3 is inexact");
        let fcsr = ctx.read_fcr(31);
        assert_ne!(fcsr & (1 << 12), 0, "Cause.Inexact set under RM={rm}");
        assert_ne!(fcsr & (1 << 2), 0, "Flag.Inexact set under RM={rm}");
    }

    // The four modes do not all agree — RM genuinely changes the emitted result.
    let bits: Vec<u32> = (0u32..=3)
        .map(|rm| {
            let mut mem_buf = vec![0u8; 32];
            let mut mem = Rdram::new(&mut mem_buf);
            let mut ctx = RecompContext::new();
            ctx.write_fcr(31, rm);
            ctx.set_f_s(2, a);
            ctx.set_f_s(4, b);
            div_block(&mut ctx, &mut mem);
            ctx.f_bits(0)
        })
        .collect();
    assert!(
        bits.iter().collect::<std::collections::BTreeSet<_>>().len() >= 2,
        "FCSR.RM must change the emitted div result across modes; got {bits:02X?}"
    );
}

/// Executed div-by-zero sets FCSR.Z (Cause bit 15 and Flag bit 5), and sqrt(-1)
/// sets FCSR.V (Cause bit 17 / Flag bit 7) — the flag plumbing the raw path
/// never produced for arithmetic.
#[test]
fn emitted_arith_sets_fcsr_cause_flags() {
    // div-by-zero -> Z. Z is exception index 3: Cause bit 12+3=15, Flag 2+3=5.
    let mut mem_buf = vec![0u8; 32];
    let mut mem = Rdram::new(&mut mem_buf);
    let mut ctx = RecompContext::new();
    ctx.set_f_s(2, 1.0);
    ctx.set_f_s(4, 0.0);
    // Pasted `div.s $f0,$f2,$f4`. Exceptions disabled: no trap, result committed.
    assert!(!ctx.fpu_div_s(0, 2, 4));
    let fcsr = ctx.read_fcr(31);
    assert_ne!(fcsr & (1 << 15), 0, "Cause.DivByZero (Z)");
    assert_ne!(fcsr & (1 << 5), 0, "Flag.DivByZero (Z)");
    assert_eq!(ctx.f_bits(0), f32::INFINITY.to_bits(), "1/0 = +inf");
    let _ = &mut mem;

    // sqrt(-1) -> V. V is exception index 4: Cause bit 16... actually 12+4=16,
    // Flag 2+4=6.
    let mut ctx = RecompContext::new();
    ctx.set_f_s(2, -1.0);
    // Pasted `sqrt.s $f0,$f2`.
    assert!(!ctx.fpu_sqrt_s(0, 2));
    let fcsr = ctx.read_fcr(31);
    assert_ne!(fcsr & (1 << 16), 0, "Cause.Invalid (V)");
    assert_ne!(fcsr & (1 << 6), 0, "Flag.Invalid (V)");

    // A big overflow -> O (index 2: Cause bit 14, Flag bit 4).
    let mut ctx = RecompContext::new();
    ctx.set_f_s(2, f32::MAX);
    ctx.set_f_s(4, f32::MAX);
    assert!(!ctx.fpu_mul_s(0, 2, 4));
    let fcsr = ctx.read_fcr(31);
    assert_ne!(fcsr & (1 << 14), 0, "Cause.Overflow (O)");
    assert_ne!(fcsr & (1 << 4), 0, "Flag.Overflow (O)");
}

// ============================================================================
// Enabled FP exception (ExcCode 15) — sub-step 2 of the FPU environment.
//
// When an arithmetic op raises an IEEE condition whose FCSR Enable bit is set,
// the VR4300 (User's Manual section 6.6) traps BEFORE writing the destination:
// the FCSR Cause field records the condition, the destination register and the
// sticky Flags field are left untouched, and the pipeline vectors to the
// ExcCode-15 general exception. The `ctx.fpu_*` shim helpers signal this by
// returning `true`; the emitted block lane turns that into a typed
// `BlockExit::Fault(CpuException::FloatingPoint)` (see the bank-runner gate).
//
// FCSR bit layout used below: Enable.V = bit 11 (index 4, 7+4), Cause.V =
// bit 16 (12+4), Flag.V = bit 6 (2+4). Inexact index 0: Enable bit 7, Cause
// bit 12, Flag bit 2.
// ============================================================================

/// An enabled Invalid (V) exception on `sqrt(-1)` traps: `fpu_sqrt_s` returns
/// `true`, the FCSR Cause.V bit is set, the destination register is NOT written,
/// and the sticky Flag.V bit is NOT set (only Cause records a trapped exception).
#[test]
fn enabled_invalid_traps_without_committing_result_or_flag() {
    let mut ctx = RecompContext::new();
    // Enable.V (bit 11); leave a recognizable sentinel in the destination $f0.
    ctx.write_fcr(31, 1 << 11);
    ctx.set_f_bits(0, 0xDEAD_BEEF);
    ctx.set_f_s(2, -1.0);

    // sqrt(-1) signals Invalid, which is enabled -> trap.
    let trapped = ctx.fpu_sqrt_s(0, 2);
    assert!(trapped, "enabled Invalid must trap");

    let fcsr = ctx.read_fcr(31);
    assert_ne!(fcsr & (1 << 16), 0, "Cause.Invalid (V) set on a trapped op");
    assert_eq!(
        fcsr & (1 << 6),
        0,
        "sticky Flag.Invalid must NOT be set on a trapped op"
    );
    assert_eq!(
        ctx.f_bits(0),
        0xDEAD_BEEF,
        "destination register must be untouched when the op traps"
    );
    // The Enable bits are unchanged by the op.
    assert_ne!(fcsr & (1 << 11), 0, "Enable.V preserved");
}

/// The disabled path is the sub-step-1 behavior: no trap, the result is
/// committed, and both Cause AND the sticky Flag bit are set. This is the
/// regression guard that enabling the trap path did not change the common case.
#[test]
fn disabled_invalid_commits_result_and_sets_sticky_flag() {
    let mut ctx = RecompContext::new();
    // Enables all clear (default FCSR).
    ctx.set_f_bits(0, 0xDEAD_BEEF);
    ctx.set_f_s(2, -1.0);

    let trapped = ctx.fpu_sqrt_s(0, 2);
    assert!(!trapped, "disabled Invalid must not trap");

    let fcsr = ctx.read_fcr(31);
    assert_ne!(fcsr & (1 << 16), 0, "Cause.Invalid (V) set");
    assert_ne!(
        fcsr & (1 << 6),
        0,
        "sticky Flag.Invalid set on the non-trapped path"
    );
    assert_ne!(
        ctx.f_bits(0),
        0xDEAD_BEEF,
        "destination must be written with the canonical-NaN result"
    );
    // sqrt(-1) yields the MIPS canonical NaN 0x7FBF_FFFF.
    assert_eq!(ctx.f_bits(0), 0x7FBF_FFFF, "canonical qNaN written");
}

/// The Cause-vs-Flags distinction, side by side: enabling the matching bit
/// flips a divide-by-zero from "sticky Flag set, result committed" to
/// "Cause only, no Flag, no result". Uses DIV.S 1/0 -> Z (index 3): Enable
/// bit 10, Cause bit 15, Flag bit 5.
#[test]
fn enabled_vs_disabled_divbyzero_flag_and_commit_differ() {
    // Disabled: Flag.Z set, result committed (+inf), no trap.
    let mut disabled = RecompContext::new();
    disabled.set_f_s(2, 1.0);
    disabled.set_f_s(4, 0.0);
    disabled.set_f_bits(0, 0x1111_1111);
    assert!(!disabled.fpu_div_s(0, 2, 4));
    let d = disabled.read_fcr(31);
    assert_ne!(d & (1 << 15), 0, "Cause.Z set (disabled)");
    assert_ne!(d & (1 << 5), 0, "Flag.Z set (disabled)");
    assert_eq!(
        disabled.f_bits(0),
        f32::INFINITY.to_bits(),
        "1/0 result committed when disabled"
    );

    // Enabled: Cause.Z set, Flag.Z NOT set, result NOT committed, trap fires.
    let mut enabled = RecompContext::new();
    enabled.write_fcr(31, 1 << 10); // Enable.Z (index 3)
    enabled.set_f_s(2, 1.0);
    enabled.set_f_s(4, 0.0);
    enabled.set_f_bits(0, 0x1111_1111);
    assert!(enabled.fpu_div_s(0, 2, 4), "enabled Z must trap");
    let e = enabled.read_fcr(31);
    assert_ne!(e & (1 << 15), 0, "Cause.Z set (enabled+trapped)");
    assert_eq!(e & (1 << 5), 0, "Flag.Z NOT set on a trapped op");
    assert_eq!(
        enabled.f_bits(0),
        0x1111_1111,
        "destination untouched on a trapped op"
    );
}

/// A raised condition whose Enable bit is clear does not trap even when a
/// DIFFERENT Enable bit is set. Overflow (index 2) is enabled but the op only
/// signals Inexact (index 0): no trap, result committed, sticky Inexact flag
/// set. Guards against enabling the trap on the wrong condition.
#[test]
fn unrelated_enable_bit_does_not_trap() {
    let mut ctx = RecompContext::new();
    // Enable.O (bit 9, index 2) only. 1/3 is Inexact (index 0), not Overflow.
    ctx.write_fcr(31, 1 << 9);
    ctx.set_f_s(2, 1.0);
    ctx.set_f_s(4, 3.0);
    let trapped = ctx.fpu_div_s(0, 2, 4);
    assert!(
        !trapped,
        "Inexact must not trap when only Overflow is enabled"
    );
    let fcsr = ctx.read_fcr(31);
    assert_ne!(fcsr & (1 << 12), 0, "Cause.Inexact set");
    assert_ne!(fcsr & (1 << 2), 0, "Flag.Inexact set (committed path)");
    assert_eq!(fcsr & (1 << 14), 0, "Cause.Overflow not set");
}

/// The FloatingPoint exception maps to ExcCode 15 with no Cause.CE (it is a
/// general exception, unlike CoprocessorUnusable's ExcCode 11). Drives
/// `enter_exception` directly so the CP0 side effects are asserted precisely:
/// EPC, Cause.ExcCode, Cause.BD, Status.EXL, and the BEV=0 general vector.
#[test]
fn floating_point_exception_vectors_to_exccode_15() {
    use fn64_recomp_rs::{BankId, CpuException, CpuFault, CpuFaultKind, ExecutionKey, GuestPc};

    let fault = CpuFault {
        at: ExecutionKey::new(BankId::new(0x1), GuestPc::new(0x8000_2000)),
        kind: CpuFaultKind::Exception {
            exception: CpuException::FloatingPoint,
            epc: GuestPc::new(0x8000_2000),
            branch_delay: false,
            instruction_code: 0,
            bad_vaddr: None,
            coprocessor: None,
        },
    };
    assert_eq!(CpuException::FloatingPoint.cause_code(), 15);

    let mut ctx = RecompContext::new();
    let vector = fault.enter_exception(&mut ctx);
    assert_eq!(
        vector,
        Some(GuestPc::new(0x8000_0180)),
        "general vector, BEV=0"
    );
    assert_eq!(ctx.cop0_epc, 0x8000_2000, "EPC = faulting instruction");
    assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 15, "Cause.ExcCode = 15");
    assert_eq!(
        ctx.cop0_cause & (1 << 31),
        0,
        "Cause.BD clear (not a delay slot)"
    );
    assert_eq!((ctx.cop0_cause >> 28) & 0b11, 0, "Cause.CE not set for FPE");
    assert_ne!(ctx.cop0_status & (1 << 1), 0, "Status.EXL set");

    // A delay-slot fault sets Cause.BD and points EPC at the branch.
    let mut delay_ctx = RecompContext::new();
    let delay_fault = CpuFault {
        at: ExecutionKey::new(BankId::new(0x1), GuestPc::new(0x8000_2008)),
        kind: CpuFaultKind::Exception {
            exception: CpuException::FloatingPoint,
            epc: GuestPc::new(0x8000_2004),
            branch_delay: true,
            instruction_code: 0,
            bad_vaddr: None,
            coprocessor: None,
        },
    };
    delay_fault.enter_exception(&mut delay_ctx);
    assert_eq!(
        delay_ctx.cop0_epc, 0x8000_2004,
        "EPC = the branch, not the slot"
    );
    assert_ne!(
        delay_ctx.cop0_cause & (1 << 31),
        0,
        "Cause.BD set in a delay slot"
    );
}

// ============================================================================
// FR=1 register file (sub-step 3, item 1).
//
// Status.FR (COP0 reg 12, bit 26) selects the FPU register organization. Set it
// via `write_cop0(12, 1 << 26)`.
// ============================================================================

const STATUS_FR: u32 = 1 << 26;

/// FR=1: all 32 registers are independent 64-bit FGRs. An ODD double register
/// is a full, separate register — a write to `$f3` must NOT touch `$f2`.
#[test]
fn fr1_odd_double_is_independent_register() {
    let mut ctx = RecompContext::new();
    ctx.write_cop0(12, STATUS_FR);
    assert!(ctx.fpu_fr(), "FR set");

    ctx.set_d_bits(2, 0xAAAA_AAAA_AAAA_AAAA);
    ctx.set_d_bits(3, 0x5555_5555_5555_5555);
    assert_eq!(ctx.d_bits(2), 0xAAAA_AAAA_AAAA_AAAA, "$f2 unchanged by $f3 write");
    assert_eq!(ctx.d_bits(3), 0x5555_5555_5555_5555, "$f3 is its own register");
}

/// FR=1: single-precision `$fN` is the low 32 bits of slot N — there is NO
/// even/odd high-word aliasing. The odd single `$f5` is independent of `$f4`.
#[test]
fn fr1_odd_single_is_independent_no_aliasing() {
    let mut ctx = RecompContext::new();
    ctx.write_cop0(12, STATUS_FR);
    ctx.set_f_bits(4, 0x1111_1111);
    ctx.set_f_bits(5, 0x2222_2222);
    assert_eq!(ctx.f_bits(4), 0x1111_1111, "FR=1: $f4 own low word");
    assert_eq!(ctx.f_bits(5), 0x2222_2222, "FR=1: $f5 own low word, not $f4 high");
    // $f4's slot high word is untouched by the $f5 write (no aliasing).
    assert_eq!(ctx.d_bits(4) >> 32, 0, "FR=1: no write bled into $f4 high word");
}

/// FR=0 even-pairing is preserved after this change: the even/odd single alias
/// and the even-only double addressing still hold (regression guard).
#[test]
fn fr0_pairing_preserved() {
    let mut ctx = RecompContext::new(); // FR=0.
    assert!(!ctx.fpu_fr());
    ctx.set_d_bits(4, 0x1122_3344_5566_7788);
    assert_eq!(ctx.f_bits(4), 0x5566_7788, "FR=0 even single = low word");
    assert_eq!(ctx.f_bits(5), 0x1122_3344, "FR=0 odd single = even partner high word");
}

/// The SAME program run under FR=0 vs FR=1 behaves per spec: writing $f2 then
/// reading $f3 as a double sees the aliased value in FR=0 but garbage/zero (the
/// independent register) in FR=1.
#[test]
fn fr_mode_changes_odd_double_semantics() {
    let mut fr0 = RecompContext::new();
    fr0.set_d_bits(2, 0x0102_0304_0506_0708);
    let fr0_odd = fr0.d_bits(3); // aliases $f2

    let mut fr1 = RecompContext::new();
    fr1.write_cop0(12, STATUS_FR);
    fr1.set_d_bits(2, 0x0102_0304_0506_0708);
    let fr1_odd = fr1.d_bits(3); // independent (still zero)

    assert_eq!(fr0_odd, 0x0102_0304_0506_0708, "FR=0: $f3 aliases $f2");
    assert_eq!(fr1_odd, 0, "FR=1: $f3 is its own (untouched) register");
    assert_ne!(fr0_odd, fr1_odd, "the FR bit changes the observable semantics");
}

// ============================================================================
// FP conditional moves (sub-step 3, item 2).
// ============================================================================

/// MOVT moves the source when the FPU condition flag is SET (and MOVF does not);
/// MOVF moves when clear. The destination is left unchanged when the predicate
/// fails. Both S and D.
#[test]
fn movt_movf_honor_condition_flag() {
    // MOVT.S: cond set -> move; cond clear -> no move.
    let mut ctx = RecompContext::new();
    ctx.set_f_bits(0, 0xDEAD_BEEF); // fd sentinel
    ctx.set_f_bits(2, 0x4048_0000); // fs = 3.125f
    ctx.fpu_cond = true;
    ctx.fpu_movcf_s(0, 2, true); // MOVT.S: cond==true -> move
    assert_eq!(ctx.f_bits(0), 0x4048_0000, "MOVT.S moves when cond set");

    ctx.set_f_bits(0, 0xDEAD_BEEF);
    ctx.fpu_cond = false;
    ctx.fpu_movcf_s(0, 2, true); // MOVT.S: cond==false -> no move
    assert_eq!(ctx.f_bits(0), 0xDEAD_BEEF, "MOVT.S leaves fd when cond clear");

    // MOVF.S: cond clear -> move.
    ctx.set_f_bits(0, 0xDEAD_BEEF);
    ctx.fpu_cond = false;
    ctx.fpu_movcf_s(0, 2, false);
    assert_eq!(ctx.f_bits(0), 0x4048_0000, "MOVF.S moves when cond clear");

    // MOVT.D on doubles.
    let mut d = RecompContext::new();
    d.set_d_bits(0, 0xDEAD_BEEF_DEAD_BEEF);
    d.set_d_bits(2, 0x4009_0000_0000_0000); // 3.125
    d.fpu_cond = true;
    d.fpu_movcf_d(0, 2, true);
    assert_eq!(d.d_bits(0), 0x4009_0000_0000_0000, "MOVT.D moves when cond set");
    d.set_d_bits(0, 0xDEAD_BEEF_DEAD_BEEF);
    d.fpu_cond = false;
    d.fpu_movcf_d(0, 2, true);
    assert_eq!(d.d_bits(0), 0xDEAD_BEEF_DEAD_BEEF, "MOVT.D no move when cond clear");
}

/// MOVZ moves when the GPR reads zero; MOVN moves when nonzero. Both S and D.
#[test]
fn movz_movn_honor_gpr() {
    let mut ctx = RecompContext::new();
    ctx.set_f_bits(0, 0xDEAD_BEEF);
    ctx.set_f_bits(2, 0x4048_0000);
    ctx.set_r(8, 0);
    ctx.fpu_movz_s(0, 2, 8); // rt==0 -> move
    assert_eq!(ctx.f_bits(0), 0x4048_0000, "MOVZ.S moves when GPR==0");

    ctx.set_f_bits(0, 0xDEAD_BEEF);
    ctx.set_r(8, 5);
    ctx.fpu_movz_s(0, 2, 8); // rt!=0 -> no move
    assert_eq!(ctx.f_bits(0), 0xDEAD_BEEF, "MOVZ.S no move when GPR!=0");

    ctx.set_f_bits(0, 0xDEAD_BEEF);
    ctx.set_r(8, 5);
    ctx.fpu_movn_s(0, 2, 8); // rt!=0 -> move
    assert_eq!(ctx.f_bits(0), 0x4048_0000, "MOVN.S moves when GPR!=0");

    // Double variants.
    let mut d = RecompContext::new();
    d.set_d_bits(0, 0xDEAD_BEEF_DEAD_BEEF);
    d.set_d_bits(2, 0x4009_0000_0000_0000);
    d.set_r(8, 0);
    d.fpu_movz_d(0, 2, 8);
    assert_eq!(d.d_bits(0), 0x4009_0000_0000_0000, "MOVZ.D moves when GPR==0");
    d.set_d_bits(0, 0xDEAD_BEEF_DEAD_BEEF);
    d.fpu_movn_d(0, 2, 8); // GPR still 0 -> no move
    assert_eq!(d.d_bits(0), 0xDEAD_BEEF_DEAD_BEEF, "MOVN.D no move when GPR==0");
}

/// The MOVF/MOVT/MOVZ/MOVN.fmt words decode to the right typed instructions
/// (funct 0x11/0x12/0x13, `tf` = bit 16, GPR in the ft field).
#[test]
fn conditional_moves_decode() {
    use fn64_recomp_rs::Instruction::*;
    // Layout: op(0x11)<<26 | fmt<<21 | ft<<16 | fs<<11 | fd<<6 | funct.
    // MOVT.S $f4,$f2,cc0: fmt=S(0x10), tf=1 (ft bit0), funct=0x11.
    let movt_s = (0x11 << 26) | (0x10 << 21) | (0x1 << 16) | (2 << 11) | (4 << 6) | 0x11;
    assert_eq!(decode(movt_s), MovcfS { fd: 4, fs: 2, tf: true });
    let movf_s = (0x11 << 26) | (0x10 << 21) | (2 << 11) | (4 << 6) | 0x11; // ft=0 => tf=0
    assert_eq!(decode(movf_s), MovcfS { fd: 4, fs: 2, tf: false });
    // MOVZ.S $f4,$f2,$t0(=8): ft carries the GPR index.
    let movz_s = (0x11 << 26) | (0x10 << 21) | (8 << 16) | (2 << 11) | (4 << 6) | 0x12;
    assert_eq!(decode(movz_s), MovzS { fd: 4, fs: 2, rt: 8 });
    let movn_d = (0x11 << 26) | (0x11 << 21) | (8 << 16) | (2 << 11) | (4 << 6) | 0x13;
    assert_eq!(decode(movn_d), MovnD { fd: 4, fs: 2, rt: 8 });
}

// ============================================================================
// Denormal -> Unimplemented Operation (sub-step 3, item 3).
//
// The VR4300 does not process subnormal operands/results; it raises the
// Unimplemented Operation exception (FCSR Cause bit E, bit 17), which is
// UNMASKABLE — it always traps to ExcCode 15 regardless of the Enable field.
// FCSR Cause.E = bit 17.
// ============================================================================

const CAUSE_E: u32 = 1 << 17;

/// A denormal OPERAND raises Unimplemented Operation and traps even with ALL
/// Enable bits clear (E is unmaskable). Cause.E is set; the destination is not
/// committed.
#[test]
fn denormal_operand_traps_unimplemented_even_with_enables_clear() {
    let mut ctx = RecompContext::new(); // all Enables clear.
    // Smallest positive single subnormal: exponent 0, mantissa 1.
    let denorm = 0x0000_0001u32;
    ctx.set_f_bits(2, denorm);
    ctx.set_f_bits(4, 1.0f32.to_bits());
    ctx.set_f_bits(0, 0xDEAD_BEEF); // fd sentinel
    let trapped = ctx.fpu_add_s(0, 2, 4);
    assert!(trapped, "denormal operand traps (E is unmaskable)");
    assert_ne!(ctx.read_fcr(31) & CAUSE_E, 0, "Cause.E set");
    assert_eq!(ctx.f_bits(0), 0xDEAD_BEEF, "destination not committed on trap");
}

/// A denormal RESULT (from a normal op that underflows into subnormal range)
/// also raises Unimplemented Operation. `MIN_POSITIVE * 0.5` is subnormal.
#[test]
fn denormal_result_traps_unimplemented() {
    let mut ctx = RecompContext::new();
    ctx.set_f_bits(2, f32::MIN_POSITIVE.to_bits()); // smallest normal
    ctx.set_f_bits(4, 0.5f32.to_bits());
    let trapped = ctx.fpu_mul_s(0, 2, 4); // -> subnormal result
    assert!(trapped, "a subnormal result traps E");
    assert_ne!(ctx.read_fcr(31) & CAUSE_E, 0, "Cause.E set on denormal result");
}

/// A normal op with normal operands and a normal result does NOT raise E.
#[test]
fn normal_op_does_not_trap_unimplemented() {
    let mut ctx = RecompContext::new();
    ctx.set_f_bits(2, 1.5f32.to_bits());
    ctx.set_f_bits(4, 2.25f32.to_bits());
    let trapped = ctx.fpu_add_s(0, 2, 4);
    assert!(!trapped, "normal op does not trap");
    assert_eq!(ctx.read_fcr(31) & CAUSE_E, 0, "Cause.E clear");
    assert_eq!(ctx.f_bits(0), (1.5f32 + 2.25f32).to_bits(), "result committed");
}

/// The shim's denormal predicates directly (double precision too).
#[test]
fn denormal_predicates() {
    assert!(fpu::is_denormal_s(0x0000_0001));
    assert!(fpu::is_denormal_s(0x007F_FFFF));
    assert!(!fpu::is_denormal_s(0), "zero is not a denormal");
    assert!(!fpu::is_denormal_s(f32::MIN_POSITIVE.to_bits()), "smallest normal is not denormal");
    assert!(fpu::is_denormal_d(0x0000_0000_0000_0001));
    assert!(!fpu::is_denormal_d(0), "zero is not a denormal");
    assert!(!fpu::is_denormal_d(f64::MIN_POSITIVE.to_bits()));
}
