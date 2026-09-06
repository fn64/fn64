//! **Property tests for the `truncf` FPU emitter, against the existing
//! oracle harness.**
//!
//! # Which emitter, and why
//!
//! `tests/fpu_oracle.rs` carries two FPU differential oracles. Counting the
//! cases each one sweeps:
//!
//! | oracle | emitter under test | cases |
//! |---|---|---|
//! | `truncf_matches_c_oracle` | `truncf` (TRUNC.W.S + CVT.S.W) | **16** |
//! | `synth_matches_oracle` | `SYNTH_WORDS` (MTC1/MUL.S/C.LT.S/...) | 8 |
//!
//! `truncf` wins, so it is the emitter this file generalizes from a
//! 16-element fixed sweep to a generated one.
//!
//! # The oracle is the existing harness, unchanged
//!
//! This file writes NO new emitter and NO new C reference. It reuses
//! `fpu_oracle.rs`'s two existing artifacts:
//!
//! - `tests/goldens/truncf.rs` -- the emitter's own output, which
//!   `truncf_emitter_output_matches_pasted_function` already pins byte-for-byte
//!   against live `emit_function`. This file `include!`s that golden, so the
//!   code it executes is the emitter's product, guarded by a test that already
//!   exists.
//! - The C-derived oracle `(float)(int32_t)x`, hand-transcribed from
//!   `aki-recomp/games/OOTU/RecompiledFuncs/funcs_56.c` in that file's module
//!   doc. [`truncf_c_oracle`] below is that same transcription.
//!
//! No C toolchain is involved: the reference is C-DERIVED but hand-transcribed
//! to Rust, and the emitted body executes in-process.
//!
//! # The special classes, and where the oracle stops
//!
//! The brief requires the random operands to include the classes the C
//! reference handles. The VR4300 does NOT treat them all as ordinary values,
//! and the runtime's own conversion (`runtime/fpu_ops.rs:315-340`,
//! `:371-383`) partitions every `f32` bit pattern into exactly four arms:
//!
//! | class | arm |
//! |---|---|
//! | finite, rounds inside `i32` | Ok(value), Inexact if it rounded |
//! | signaling NaN | Invalid, result `i32::MAX` |
//! | quiet NaN, subnormal, infinity | **Unimplemented** -- traps |
//! | finite, rounds outside `i32` | **Unimplemented** -- traps |
//!
//! The C oracle `(int32_t)x` is only a faithful reference on the FIRST arm.
//! On the other three the hardware raises an exception where C has undefined
//! behaviour, so asserting C equality there would assert something the
//! reference does not actually say.
//!
//! So this file splits the domain rather than papering over it:
//!
//! - `the_emitted_truncf_matches_the_c_oracle_on_convertible_operands` --
//!   the C differential, on the arm where C is a valid reference.
//! - `every_float_bit_pattern_lands_in_exactly_one_documented_arm` -- a
//!   totality property over ARBITRARY 32-bit patterns, including every
//!   special class. **Every arm runs the conversion and asserts the concrete
//!   value the reference specifies** -- the convertible arm its truncation,
//!   the sNaN arm `i32::MAX`, and the two Unimplemented arms the exception
//!   itself. This is what covers zero, negative zero, subnormals, infinities
//!   and NaN payloads.
//!
//! # The vacuity trap
//!
//! A generator drawn from `any::<u32>()` reinterpreted as `f32` is
//! overwhelmingly NaN, infinity, or astronomically out of range -- so the C
//! differential would be filtered down to almost nothing while still
//! reporting green. `the_generator_reaches_every_special_class` counts, per
//! class, how many operands the mixed strategy produces, and fails unless
//! every class named in the brief is populated AND the convertible arm has a
//! substantial share.
//!
//! # Mutation results (see the task report for the full table)
//!
//! | mutation | killed by |
//! |---|---|
//! | `rounded_for_mode` mode 1 `trunc()` -> `floor()` (branch swap) | C differential |
//! | `try_fpu_to_i32_raw` range low `-2_147_483_648.0` -> `-2_147_483_647.0` (boundary) | totality |
//! | `try_fpu_to_i32_raw` sNaN result `i32::MAX` -> `i32::MIN` | totality (sNaN arm) |
//!
//! # Blast radius
//!
//! These properties see the conversion's rounding mode, its range gate, its
//! NaN classification and the CVT.S.W back-conversion. They do NOT see the
//! DECODE of the ROM words into those operations -- the golden pin covers
//! that -- nor the FCSR flag bits, which the existing typed tests in
//! `fpu_oracle.rs` own.

use proptest::prelude::*;

use fn64_cpu_runtime::{Rdram, RecompContext};

/// The emitter's output for `truncf`, included from the golden that
/// `fpu_oracle.rs::truncf_emitter_output_matches_pasted_function` already
/// pins byte-for-byte against live `emit_function`.
///
/// Including the golden rather than re-pasting it means this file cannot
/// drift from the emitter independently: if the emitter changes, that
/// existing test fails and this file picks the change up automatically.
#[allow(unused, clippy::all)]
mod emitted {
    use fn64_cpu_runtime::{Rdram, RecompContext};
    include!("goldens/truncf.rs");
}

/// **The oracle.** `truncf`'s recompiled C body, hand-transcribed:
///
/// ```c
/// ctx->f12.u32l = TRUNC_W_S(ctx->f12.fl);   // (int32_t)v
/// ctx->f0.fl    = CVT_S_W(ctx->f12.u32l);   // (float)(int32_t)v
/// ```
///
/// Valid only where `(int32_t)x` is defined -- see the module doc.
fn truncf_c_oracle(x: f32) -> u32 {
    let truncated: i32 = x as i32;
    (truncated as f32).to_bits()
}

/// The four documented arms of the float-to-fixed conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arm {
    /// Finite and rounds inside `i32`: the C oracle applies.
    Convertible,
    /// Signaling NaN: Invalid, result `i32::MAX`.
    SignalingNan,
    /// Quiet NaN, subnormal, or infinity: Unimplemented.
    UnimplementedOperand,
    /// Finite but rounds outside `i32`: Unimplemented.
    OutOfRange,
}

/// Classifies a bit pattern by the runtime's own documented predicates, from
/// `runtime/fpu_ops.rs`. Written from the documented rule, not by calling the
/// runtime, so it can serve as the totality oracle.
fn classify(bits: u32) -> Arm {
    let value = f32::from_bits(bits);
    let exponent = (bits >> 23) & 0xff;
    let mantissa = bits & 0x7f_ffff;

    let is_nan = exponent == 0xff && mantissa != 0;
    if is_nan {
        // **The VR4300 uses the LEGACY NaN convention**, opposite the modern
        // IEEE host one: fraction MSB SET denotes SIGNALING, per the User's
        // Manual p.151 as cited at `runtime/fpu_ops.rs:10-12`.
        //
        // This transcription originally had it backwards -- the modern
        // convention -- and the property caught it: `0x7FC00000` (MSB set,
        // "quiet" on a host) was predicted to raise Unimplemented but the
        // kernel produced `Ok(i32::MAX)`, the signaling result. The kernel is
        // right and the oracle was wrong; corrected here.
        //
        // A NaN whose remaining fraction bits are all zero is not `is_qnan32`
        // either (that predicate masks `0x003F_FFFF`), so it reaches neither
        // exceptional arm and converts like a signaling NaN.
        return if mantissa & 0x40_0000 != 0 {
            Arm::SignalingNan
        } else {
            Arm::UnimplementedOperand
        };
    }
    let is_subnormal = exponent == 0 && mantissa != 0;
    if is_subnormal || value.is_infinite() {
        return Arm::UnimplementedOperand;
    }
    // TRUNC.W.S rounds toward zero, then the range gate is a half-open
    // interval on the ROUNDED value.
    let rounded = f64::from(value).trunc();
    if !(-2_147_483_648.0..2_147_483_648.0).contains(&rounded) {
        return Arm::OutOfRange;
    }
    Arm::Convertible
}

/// Runs the emitted `truncf` body on one operand and returns `$f0`'s bits.
fn run_emitted(x: f32) -> u32 {
    let mut buffer = vec![0u8; 64];
    let mut memory = Rdram::new(&mut buffer);
    let mut ctx = RecompContext::new();
    ctx.set_f_s(12, x);
    emitted::truncf_recomp(&mut ctx, &mut memory);
    ctx.f_bits(0)
}

/// Operands the C reference explicitly handles, per the brief: zero, negative
/// zero, subnormals, infinities, NaN payloads (both quiet and signaling), and
/// the rounding/range boundary values.
const INTERESTING: &[u32] = &[
    0x0000_0000, // +0.0
    0x8000_0000, // -0.0
    0x0000_0001, // smallest positive subnormal
    0x800F_FFFF, // negative subnormal
    0x007F_FFFF, // largest subnormal
    0x0080_0000, // smallest positive normal
    0x7F80_0000, // +inf
    0xFF80_0000, // -inf
    0x7FC0_0000, // fraction MSB set: SIGNALING on the VR4300 (legacy convention)
    0x7FFF_FFFF, // fraction MSB set, saturated payload: signaling
    0x7F80_0001, // fraction MSB clear: QUIET on the VR4300 -> Unimplemented
    0xFF80_0001, // fraction MSB clear, negative: quiet -> Unimplemented
    0x3F00_0000, // 0.5   -- rounds to 0 toward zero
    0xBF00_0000, // -0.5
    0x3FC0_0000, // 1.5
    0xBFC0_0000, // -1.5
    0x3F80_0000, // 1.0
    0xBF80_0000, // -1.0
    0x4EFF_FFFF, // just below 2^31 -- the last convertible magnitude
    0x4F00_0000, // exactly 2^31    -- the first out-of-range magnitude
    0xCF00_0000, // exactly -2^31   -- convertible, the range gate's low end
    0xCF00_0001, // just past -2^31 -- out of range
    0x7F7F_FFFF, // f32::MAX
    0xFF7F_FFFF, // -f32::MAX
];

/// The mixed strategy the brief asks for: an interesting-values list unioned
/// with uniformly random bit patterns.
fn operand_bits() -> impl Strategy<Value = u32> {
    prop_oneof![
        // Weighted toward the curated list, because uniform random bits are
        // almost never convertible -- see the vacuity note in the module doc.
        3 => proptest::sample::select(INTERESTING.to_vec()),
        2 => any::<u32>(),
        // Bit patterns whose exponent is constrained to the convertible band,
        // so the C differential has a well-populated domain.
        3 => (0u32..=1, 100u32..=157, 0u32..0x80_0000)
            .prop_map(|(sign, exponent, mantissa)| (sign << 31) | (exponent << 23) | mantissa),
    ]
}

proptest! {
    /// **The C differential.** On the arm where `(int32_t)x` is a defined
    /// reference, the emitted body's `$f0` must equal the C oracle's bits
    /// exactly.
    #[test]
    fn the_emitted_truncf_matches_the_c_oracle_on_convertible_operands(
        bits in operand_bits(),
    ) {
        prop_assume!(classify(bits) == Arm::Convertible);
        let x = f32::from_bits(bits);
        let got = run_emitted(x);
        let expected = truncf_c_oracle(x);
        prop_assert_eq!(
            got,
            expected,
            "truncf divergence for {:#010X} ({}): emitter {:#010X}, C oracle {:#010X}",
            bits, x, got, expected
        );
    }

    /// **Totality, and every arm asserts a concrete produced value.**
    ///
    /// Each of the four arms RUNS the conversion the emitted body performs and
    /// asserts the result the reference specifies -- none of them merely
    /// re-states `classify`'s own predicate.
    ///
    /// **Why the exceptional arms use `try_fpu_to_i32_s` rather than
    /// `run_emitted`.** The emitted whole-function lane converts an enabled
    /// exception into a loud trap (`trap_unsupported`), so calling the whole
    /// body on an sNaN or an infinity panics instead of returning a value --
    /// measured, not assumed: probing `0x7F80_0001` through `run_emitted`
    /// aborts in `runtime/host.rs`. `try_fpu_to_i32_s` is the exact operation
    /// the emitted body invokes (the golden's first instruction is
    /// `ctx.fpu_to_i32_s(12, Some(1))`, the trapping wrapper around it), and
    /// it returns the typed result instead of trapping. So the exceptional
    /// arms assert the conversion's real product at the only level where that
    /// product is observable.
    ///
    /// An earlier version asserted only `value.is_nan()` and the quiet-bit
    /// position in the `SignalingNan` arm -- a restatement of `classify`,
    /// which never ran the kernel. A mutant changing the sNaN result from
    /// `i32::MAX` to `i32::MIN` survived it. The arm below pins that result.
    #[test]
    fn every_float_bit_pattern_lands_in_exactly_one_documented_arm(
        bits in operand_bits(),
    ) {
        let arm = classify(bits);
        let value = f32::from_bits(bits);

        // The typed conversion, at TRUNC.W.S's rounding mode (1 = toward
        // zero) -- the same `Some(1)` the emitted golden passes.
        let mut ctx = RecompContext::new();
        ctx.set_f_bits(12, bits);
        let typed = ctx.try_fpu_to_i32_s(12, Some(1));

        match arm {
            Arm::Convertible => {
                // The defining postcondition of this arm: the truncated value
                // is representable, so the roundtrip through i32 is exact.
                let truncated = value as i32;
                prop_assert!(
                    f64::from(value).trunc() == f64::from(truncated),
                    "convertible operand {:#010X} lost its value through i32", bits
                );
                // The whole emitted body agrees with the C reference...
                prop_assert_eq!(run_emitted(value), truncf_c_oracle(value));
                // ...and the conversion itself produces the truncated integer.
                prop_assert_eq!(
                    typed,
                    Ok(truncated),
                    "convertible operand {:#010X} did not convert to its truncation", bits
                );
            }
            Arm::SignalingNan => {
                // The documented sNaN result: Invalid is recorded and the
                // conversion yields i32::MAX. This is the value the mutant
                // `Ok(i32::MAX) -> Ok(i32::MIN)` changes, and asserting it is
                // what kills that mutant.
                prop_assert_eq!(
                    typed,
                    Ok(i32::MAX),
                    "sNaN {:#010X} did not produce the documented i32::MAX", bits
                );
            }
            Arm::UnimplementedOperand => {
                // Quiet NaN, subnormal and infinity all raise Unimplemented,
                // so the typed conversion must report the exception rather
                // than inventing a value.
                prop_assert!(
                    typed.is_err(),
                    "unimplemented operand {:#010X} produced {:?} instead of an exception",
                    bits, typed
                );
            }
            Arm::OutOfRange => {
                // A finite value whose truncation escapes i32 is likewise
                // Unimplemented -- the range gate, asserted through its
                // observable effect rather than by restating the constant.
                prop_assert!(
                    typed.is_err(),
                    "out-of-range operand {:#010X} produced {:?} instead of an exception",
                    bits, typed
                );
            }
        }
    }
}

/// **The anti-vacuity guard.** Every special class named in the brief must be
/// produced by the mixed strategy, and the convertible arm -- the only one the
/// C differential can assert on -- must have a substantial share.
///
/// Without this, a strategy dominated by `any::<u32>()` would filter the C
/// differential down to a handful of cases while still reporting green, which
/// is the same shape of vacuity Task 5.1's `stepping_differential` hit.
#[test]
fn the_generator_reaches_every_special_class() {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = operand_bits();

    let (mut convertible, mut snan, mut unimplemented, mut out_of_range) = (0usize, 0, 0, 0);
    let (mut zero, mut negative_zero, mut subnormal, mut infinite, mut quiet_nan) =
        (0usize, 0, 0, 0, 0);

    const CASES: usize = 4096;
    for _ in 0..CASES {
        let bits = strategy
            .new_tree(&mut runner)
            .expect("operand strategy produces a value")
            .current();
        match classify(bits) {
            Arm::Convertible => convertible += 1,
            Arm::SignalingNan => snan += 1,
            Arm::UnimplementedOperand => unimplemented += 1,
            Arm::OutOfRange => out_of_range += 1,
        }
        let value = f32::from_bits(bits);
        let exponent = (bits >> 23) & 0xff;
        let mantissa = bits & 0x7f_ffff;
        if bits == 0 {
            zero += 1;
        }
        if bits == 0x8000_0000 {
            negative_zero += 1;
        }
        if exponent == 0 && mantissa != 0 {
            subnormal += 1;
        }
        if value.is_infinite() {
            infinite += 1;
        }
        // Legacy convention: fraction MSB CLEAR is the quiet NaN.
        if value.is_nan() && mantissa & 0x40_0000 == 0 {
            quiet_nan += 1;
        }
    }

    // Every arm of the conversion must be exercised.
    assert!(
        convertible > 0 && snan > 0 && unimplemented > 0 && out_of_range > 0,
        "generator missed a conversion arm over {CASES} cases: convertible={convertible} \
         snan={snan} unimplemented={unimplemented} out_of_range={out_of_range}"
    );
    // Every special class the brief names must appear.
    assert!(
        zero > 0 && negative_zero > 0 && subnormal > 0 && infinite > 0 && quiet_nan > 0,
        "generator missed a special class over {CASES} cases: zero={zero} \
         negative_zero={negative_zero} subnormal={subnormal} infinite={infinite} \
         quiet_nan={quiet_nan}"
    );
    // The convertible arm carries the C differential; a thin share there means
    // the headline property is nearly vacuous even though it passes.
    assert!(
        convertible * 4 > CASES,
        "too few convertible operands for the C differential: {convertible} of {CASES}"
    );
}
