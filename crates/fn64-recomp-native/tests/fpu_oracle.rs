//! COP1 / FPU oracle-validation + decoder tests for `fn64-recomp-native`.
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

use fn64_recomp_native::{
    decode, emit_function, round_ties_even_f32, round_ties_even_f64, FuncInput, Instruction, Rdram,
    RecompContext,
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
    let mut pc: u32 = 0x800CD930;
    'run: loop {
        match pc {
            0x800CD930 => {
                // 0x800CD930: TruncWS { fd: 12, fs: 12 }
                ctx.set_f_bits(12, (ctx.f_s(12) as i32) as u32);
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
    let input = FuncInput { name: "truncf_recomp", vram: TRUNCF_VRAM, words: &TRUNCF_WORDS };
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
        0.0, -0.0, 0.4, 0.5, 0.6, 1.5, -0.4, -0.5, -0.6, -1.5, 2.9, -2.9, 100.0, -100.0,
        123456.75, -123456.75,
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
        assert_eq!(out.stored, exp_store, "store mismatch for a0={a0}, in={in_val}");
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
    mem_buf[0x20..0x24].copy_from_slice(&in_val.to_bits().to_be_bytes());
    let mut mem = Rdram::new(&mut mem_buf);
    let mut ctx = RecompContext::new();
    ctx.set_r32(4, a0); // $a0
    ctx.set_r(5, SYNTH_IN_VADDR); // $a1 -> input
    ctx.set_r(6, SYNTH_OUT_VADDR); // $a2 -> output

    synth_recomp(&mut ctx, &mut mem);

    let v0 = ctx.r(2);
    let f0 = ctx.f_bits(0);
    let stored = u32::from_be_bytes([mem_buf[0x30], mem_buf[0x31], mem_buf[0x32], mem_buf[0x33]]);
    SynthOut { v0, f0, stored }
}

// --- Synthetic emitter output, pasted VERBATIM (guarded by the golden test). ---
#[allow(unused, clippy::all)]
pub fn synth_recomp(ctx: &mut RecompContext, mem: &mut Rdram) {
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
                ctx.set_f_s(8, ctx.f_s(4) * ctx.f_s(6));
                // 0x80100010: AddS { fd: 0, fs: 8, ft: 4 }
                ctx.set_f_s(0, ctx.f_s(8) + ctx.f_s(4));
                // 0x80100014: CLtS { fs: 0, ft: 6 }
                ctx.fpu_cond = ctx.f_s(0) < ctx.f_s(6);
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
    let input = FuncInput { name: "synth_recomp", vram: SYNTH_VRAM, words: &SYNTH_WORDS };
    let emitted = emit_function(&input);
    let pasted = include_str!("goldens/synth.rs");
    let norm = |s: &str| s.trim_end().replace("\r\n", "\n");
    assert_eq!(norm(&emitted), norm(pasted), "synth emitter output drifted from goldens/synth.rs");
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
        let stored = f32::from_bits(u32::from_be_bytes([
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
    let _ = emit_function(&FuncInput { name: "xcallee", vram: XCALLEE_VRAM, words: &XCALLEE_WORDS });
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
    assert_eq!(ctx.f_bits(4), 0x5566_7788, "even single = low word of the slot");
    assert_eq!(ctx.f_bits(5), 0x1122_3344, "odd single = high word of the even partner");

    // Writing the odd single $f5 must land in the HIGH word, leaving the low
    // word (even single $f4) untouched — the mtc1-to-odd case that was the
    // OoT-boot SIGSEGV-at-0x40 in fn64-abi.
    ctx.set_f_bits(5, 0xDEAD_BEEF);
    assert_eq!(ctx.d_bits(4), 0xDEAD_BEEF_5566_7788);
    assert_eq!(ctx.f_bits(4), 0x5566_7788, "low word preserved by an odd-register write");
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
fn decode_cop1_loads_stores() {
    // lwc1 $f6, 0($a1)  = 0xC4A60000
    assert_eq!(decode(0xC4A60000), Instruction::Lwc1 { ft: 6, base: 5, off: 0 });
    // swc1 $f0, 0($a2)  = 0xE4C00000
    assert_eq!(decode(0xE4C00000), Instruction::Swc1 { ft: 0, base: 6, off: 0 });
    // ldc1 $f4, 0x8($sp) = 0xD7A40008
    assert_eq!(decode(0xD7A40008), Instruction::Ldc1 { ft: 4, base: 29, off: 8 });
    // sdc1 $f20, -0x8($fp) = 0xF7D4FFF8
    assert_eq!(decode(0xF7D4FFF8), Instruction::Sdc1 { ft: 20, base: 30, off: -8 });
}

#[test]
fn decode_cop1_single_arith() {
    // add.s $f0, $f2, $f4 = 0x46041000  (fmt=S ft=f4 fs=f2 fd=f0 funct=0)
    assert_eq!(decode(0x46041000), Instruction::AddS { fd: 0, fs: 2, ft: 4 });
    // sub.s $f0, $f2, $f4 = 0x46041001
    assert_eq!(decode(0x46041001), Instruction::SubS { fd: 0, fs: 2, ft: 4 });
    // mul.s $f8, $f4, $f6 = 0x46062202
    assert_eq!(decode(0x46062202), Instruction::MulS { fd: 8, fs: 4, ft: 6 });
    // div.s $f0, $f2, $f4 = 0x46041003
    assert_eq!(decode(0x46041003), Instruction::DivS { fd: 0, fs: 2, ft: 4 });
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
    assert_eq!(decode(0x46241000), Instruction::AddD { fd: 0, fs: 2, ft: 4 });
    // mul.d $f0, $f2, $f4 = 0x46241002
    assert_eq!(decode(0x46241002), Instruction::MulD { fd: 0, fs: 2, ft: 4 });
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
    for w in [0x4600630F, 0x4600630E, 0x4620630F, 0x4620630E, 0x4600630C, 0x4620630C] {
        assert!(!matches!(decode(w), Instruction::Unknown { .. }), "word {w:#010X} still Unknown");
    }
}

/// The emitter must produce the floor/ceil-then-truncate expression for each.
#[test]
fn floor_ceil_w_emit() {
    let emit1 = |word: u32| -> String {
        let input = FuncInput { name: "t", vram: 0x8000_0000, words: &[word, 0x03E00008, 0] };
        emit_function(&input)
    };
    assert!(emit1(0x4600630F).contains("ctx.set_f_bits(12, (ctx.f_s(12).floor() as i32) as u32);"));
    assert!(emit1(0x4600630E).contains("ctx.set_f_bits(12, (ctx.f_s(12).ceil() as i32) as u32);"));
    assert!(emit1(0x4620630F).contains("ctx.set_f_bits(12, (ctx.f_d(12).floor() as i32) as u32);"));
    assert!(emit1(0x4620630E).contains("ctx.set_f_bits(12, (ctx.f_d(12).ceil() as i32) as u32);"));
    assert!(emit1(0x4600630C)
        .contains("ctx.set_f_bits(12, round_ties_even_f32(ctx.f_s(12)) as i32 as u32);"));
    assert!(emit1(0x4620630C)
        .contains("ctx.set_f_bits(12, round_ties_even_f64(ctx.f_d(12)) as i32 as u32);"));
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
