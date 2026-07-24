//! IEEE-754 soft-float shim for VR4300 COP1 arithmetic.
//!
//! # Why this exists
//!
//! Host `f32`/`f64` arithmetic always rounds to nearest and yields no IEEE
//! exception flags, so it cannot model the VR4300 FPU: the guest can select any
//! of the four FCSR rounding modes (RN/RZ/RP/RM) and reads back exception
//! Cause/Flag bits after every op. This module performs each arithmetic op
//! through [`rustc_apfloat`] — the same IEEE-754 core `rustc` trusts — which is
//! bit-exact *and host-independent* (the `wrong==0` discipline requires the
//! latter: two different host CPUs must produce identical bits).
//!
//! Each public function takes the operand(s) as raw register bits plus the
//! two-bit FCSR rounding mode, and returns `(result_bits, `[`Flags`]`)` — the
//! result already materialized as MIPS register bits (including the VR4300
//! *legacy-encoding* canonical NaN, which differs from apfloat's IEEE-2008
//! encoding) and the exception flags the caller feeds through `raise_fpu`.
//!
//! # Scope (this sub-step)
//!
//! Add/Sub/Mul/Div/Sqrt/Abs/Neg for single and double, honoring FCSR.RM and
//! producing Invalid/DivByZero/Overflow/Underflow/Inexact flags. This module is
//! pure (no FCSR state, no trapping): the caller owns FCSR and the enabled-
//! exception decision. Denormal→Unimplemented-Operation trapping and enabled-
//! exception (ExcCode-15) vectoring are deliberately *not* here — see the
//! module-level notes in `runtime.rs` and the design spec for the next steps.

use rustc_apfloat::ieee::{Double, Single};
use rustc_apfloat::{Float, Round, Status, StatusAnd};

/// The five VR4300 FPU exception conditions, in the bit order [`raise_fpu`]
/// (`runtime.rs`) expects: index = the `exception` argument it takes.
///
/// A raised flag means "this op signaled this IEEE condition". The caller ORs
/// each set flag into FCSR Cause and the sticky Flags field, and — for a later
/// sub-step — vectors if the matching Enable bit is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    /// Inexact (I) — the result was rounded. FCSR exception index 0.
    pub inexact: bool,
    /// Underflow (U) — a tiny nonzero result. FCSR exception index 1.
    pub underflow: bool,
    /// Overflow (O) — magnitude exceeded the format's largest finite. Index 2.
    pub overflow: bool,
    /// Division by zero (Z) — finite nonzero / zero. FCSR exception index 3.
    pub divbyzero: bool,
    /// Invalid operation (V) — e.g. 0*inf, inf-inf, sqrt(-x), SNaN input.
    /// FCSR exception index 4.
    pub invalid: bool,
}

impl Flags {
    const NONE: Flags = Flags {
        inexact: false,
        underflow: false,
        overflow: false,
        divbyzero: false,
        invalid: false,
    };
}

/// Map an apfloat [`Status`] bitset onto MIPS FCSR [`Flags`].
///
/// The apfloat status is IEEE-exact; the field-for-field correspondence is
/// direct. apfloat always ORs INEXACT into OVERFLOW/UNDERFLOW results, matching
/// the VR4300 (an overflowed or underflowed result is also inexact), so we
/// carry INEXACT through verbatim rather than suppressing it.
fn flags_from_status(status: Status) -> Flags {
    Flags {
        inexact: status.contains(Status::INEXACT),
        underflow: status.contains(Status::UNDERFLOW),
        overflow: status.contains(Status::OVERFLOW),
        divbyzero: status.contains(Status::DIV_BY_ZERO),
        invalid: status.contains(Status::INVALID_OP),
    }
}

/// Translate the 2-bit FCSR.RM field to apfloat's rounding attribute.
/// FCSR.RM: 0=RN (nearest, ties to even), 1=RZ (toward zero), 2=RP (toward
/// +inf), 3=RM (toward -inf) — VR4300 User's Manual section 6.3.2.1.
fn round_of(fcsr_rm: u8) -> Round {
    match fcsr_rm & 3 {
        0 => Round::NearestTiesToEven,
        1 => Round::TowardZero,
        2 => Round::TowardPositive,
        3 => Round::TowardNegative,
        _ => unreachable!("FCSR.RM is two bits"),
    }
}

// ---------------------------------------------------------------------------
// MIPS legacy-encoding canonical NaN.
//
// The VR4300 uses the *legacy* MIPS NaN convention: the quiet bit is the MSB of
// the trailing significand and is CLEAR for a quiet NaN (the opposite of the
// IEEE-754-2008 encoding apfloat uses). A quiet NaN result is always emitted as
// the single canonical pattern below, regardless of any input NaN payload.
// ---------------------------------------------------------------------------

/// VR4300 canonical single-precision quiet NaN: exponent all ones, quiet bit
/// (mantissa MSB) clear, remaining mantissa bits set.
const CANON_QNAN_S: u32 = 0x7FBF_FFFF;
/// VR4300 canonical double-precision quiet NaN.
const CANON_QNAN_D: u64 = 0x7FF7_FFFF_FFFF_FFFF;

/// A single-precision NaN whose signaling bit (legacy MIPS: mantissa MSB set)
/// marks it as an SNaN. Mirrors `runtime::is_snan32`.
fn is_snan_s(bits: u32) -> bool {
    bits & 0x7F80_0000 == 0x7F80_0000 && bits & 0x007F_FFFF != 0 && bits & 0x0040_0000 == 0
}

/// Double-precision counterpart of [`is_snan_s`]. Mirrors `runtime::is_snan64`.
fn is_snan_d(bits: u64) -> bool {
    bits & 0x7FF0_0000_0000_0000 == 0x7FF0_0000_0000_0000
        && bits & 0x000F_FFFF_FFFF_FFFF != 0
        && bits & 0x0008_0000_0000_0000 == 0
}

/// Rewrite an apfloat result into MIPS register bits: if it is a NaN, replace it
/// with the VR4300 canonical quiet NaN; otherwise keep the exact bits.
fn canon_s(value: Single) -> u32 {
    let bits = value.to_bits() as u32;
    if value.is_nan() {
        CANON_QNAN_S
    } else {
        bits
    }
}

/// Double-precision counterpart of [`canon_s`].
fn canon_d(value: Double) -> u64 {
    let bits = value.to_bits() as u64;
    if value.is_nan() {
        CANON_QNAN_D
    } else {
        bits
    }
}

/// If either operand is a signaling NaN, the op is Invalid and the result is the
/// canonical quiet NaN — the shared front half of every binary arithmetic op.
/// Returns `Some((canonical_nan_bits, flags))` when a short-circuit applies.
fn snan_short_circuit_s(a: u32, b: u32) -> Option<(u32, Flags)> {
    if is_snan_s(a) || is_snan_s(b) {
        Some((
            CANON_QNAN_S,
            Flags {
                invalid: true,
                ..Flags::NONE
            },
        ))
    } else {
        None
    }
}

fn snan_short_circuit_d(a: u64, b: u64) -> Option<(u64, Flags)> {
    if is_snan_d(a) || is_snan_d(b) {
        Some((
            CANON_QNAN_D,
            Flags {
                invalid: true,
                ..Flags::NONE
            },
        ))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Single-precision binary arithmetic.
//
// Each op decodes the operand bits into apfloat `Single`s, runs the op under the
// requested rounding mode, translates the status flags, and re-encodes the
// result (canonicalizing a NaN). apfloat already raises INVALID_OP for the
// arithmetic invalidities (inf-inf, 0*inf, x/0 where x is 0 or inf) and
// DIV_BY_ZERO for finite/0, so we only special-case a signaling-NaN operand
// (which apfloat quiets under its own encoding — we want the MIPS flag + bits).
// ---------------------------------------------------------------------------

/// `fd = fs + ft` (single). Honors FCSR.RM; returns the MIPS result bits and
/// IEEE flags.
pub fn add_s(a: u32, b: u32, fcsr_rm: u8) -> (u32, Flags) {
    if let Some(sc) = snan_short_circuit_s(a, b) {
        return sc;
    }
    let StatusAnd { status, value } =
        Single::from_bits(a as u128).add_r(Single::from_bits(b as u128), round_of(fcsr_rm));
    (canon_s(value), flags_from_status(status))
}

/// `fd = fs - ft` (single).
pub fn sub_s(a: u32, b: u32, fcsr_rm: u8) -> (u32, Flags) {
    if let Some(sc) = snan_short_circuit_s(a, b) {
        return sc;
    }
    let StatusAnd { status, value } =
        Single::from_bits(a as u128).sub_r(Single::from_bits(b as u128), round_of(fcsr_rm));
    (canon_s(value), flags_from_status(status))
}

/// `fd = fs * ft` (single).
pub fn mul_s(a: u32, b: u32, fcsr_rm: u8) -> (u32, Flags) {
    if let Some(sc) = snan_short_circuit_s(a, b) {
        return sc;
    }
    let StatusAnd { status, value } =
        Single::from_bits(a as u128).mul_r(Single::from_bits(b as u128), round_of(fcsr_rm));
    (canon_s(value), flags_from_status(status))
}

/// `fd = fs / ft` (single).
pub fn div_s(a: u32, b: u32, fcsr_rm: u8) -> (u32, Flags) {
    if let Some(sc) = snan_short_circuit_s(a, b) {
        return sc;
    }
    let StatusAnd { status, value } =
        Single::from_bits(a as u128).div_r(Single::from_bits(b as u128), round_of(fcsr_rm));
    (canon_s(value), flags_from_status(status))
}

// ---------------------------------------------------------------------------
// Double-precision binary arithmetic (mirror of the single-precision block).
// ---------------------------------------------------------------------------

/// `fd = fs + ft` (double).
pub fn add_d(a: u64, b: u64, fcsr_rm: u8) -> (u64, Flags) {
    if let Some(sc) = snan_short_circuit_d(a, b) {
        return sc;
    }
    let StatusAnd { status, value } =
        Double::from_bits(a as u128).add_r(Double::from_bits(b as u128), round_of(fcsr_rm));
    (canon_d(value), flags_from_status(status))
}

/// `fd = fs - ft` (double).
pub fn sub_d(a: u64, b: u64, fcsr_rm: u8) -> (u64, Flags) {
    if let Some(sc) = snan_short_circuit_d(a, b) {
        return sc;
    }
    let StatusAnd { status, value } =
        Double::from_bits(a as u128).sub_r(Double::from_bits(b as u128), round_of(fcsr_rm));
    (canon_d(value), flags_from_status(status))
}

/// `fd = fs * ft` (double).
pub fn mul_d(a: u64, b: u64, fcsr_rm: u8) -> (u64, Flags) {
    if let Some(sc) = snan_short_circuit_d(a, b) {
        return sc;
    }
    let StatusAnd { status, value } =
        Double::from_bits(a as u128).mul_r(Double::from_bits(b as u128), round_of(fcsr_rm));
    (canon_d(value), flags_from_status(status))
}

/// `fd = fs / ft` (double).
pub fn div_d(a: u64, b: u64, fcsr_rm: u8) -> (u64, Flags) {
    if let Some(sc) = snan_short_circuit_d(a, b) {
        return sc;
    }
    let StatusAnd { status, value } =
        Double::from_bits(a as u128).div_r(Double::from_bits(b as u128), round_of(fcsr_rm));
    (canon_d(value), flags_from_status(status))
}

// ---------------------------------------------------------------------------
// Square root.
//
// apfloat 0.2.3 does not implement sqrt (its trait lists it as a future op), so
// we compute the *correctly rounded* IEEE square root ourselves and validate it
// with apfloat's exact arithmetic — no host-dependent shortcut.
//
// Algorithm (see the Tuckerman test / arxiv 2404.00387): for a finite positive
// input x with a candidate result y (an F-value close to sqrt(x)), the exact
// residual sign of (y*y - x) tells us whether y is high, low, or exact. A
// crucial fact: for square root, the exact result is NEVER a rounding midpoint
// when input and result share a format — so round-to-nearest has no ties, and a
// single ULP correction using the exact residual gives the correctly rounded
// value in every mode. We compute the exact product in a WIDER apfloat format
// (Double is exact for a Single candidate; a u128-precision widen is exact for a
// Double candidate) so the residual sign is exact.
// ---------------------------------------------------------------------------

/// Sign of the exact residual `y*y - x`, both promoted losslessly to `Double`.
/// Because `y` is a `Single` (24-bit significand), `y*y` has ≤48 significant
/// bits and `Double` (53-bit) holds it exactly; `x` (24-bit) is exact too.
/// Returns `Less`/`Equal`/`Greater` for `y*y` vs `x`.
fn residual_cmp_s(y: Single, x: Single) -> core::cmp::Ordering {
    let y_wide = widen_s_to_d(y);
    let x_wide = widen_s_to_d(x);
    // y*y is exact in Double (48 < 53 bits); subtraction of two exact values
    // that differ by <1 ulp of the wider format is likewise exact here.
    let prod = y_wide.mul_r(y_wide, Round::NearestTiesToEven).value;
    prod.partial_cmp(&x_wide).expect("finite operands compare")
}

/// Losslessly promote a finite `Single` to `Double` (widening never rounds).
fn widen_s_to_d(v: Single) -> Double {
    // Route through the concrete f32→f64 bit space: every finite/zero f32 is an
    // exact f64. (Used only for finite, non-NaN values in the sqrt path.)
    let f = f32::from_bits(v.to_bits() as u32);
    Double::from_bits((f as f64).to_bits() as u128)
}

/// `fd = sqrt(fs)` (single), correctly rounded under FCSR.RM.
///
/// Special cases follow IEEE-754 / VR4300: `sqrt(+0)=+0`, `sqrt(-0)=-0`,
/// `sqrt(+inf)=+inf`, a quiet-NaN input propagates the canonical NaN, and any
/// negative finite/infinite input (or an SNaN input) is Invalid → canonical
/// quiet NaN.
pub fn sqrt_s(a: u32, fcsr_rm: u8) -> (u32, Flags) {
    let x = Single::from_bits(a as u128);
    // SNaN input: Invalid, canonical quiet NaN.
    if is_snan_s(a) {
        return (
            CANON_QNAN_S,
            Flags {
                invalid: true,
                ..Flags::NONE
            },
        );
    }
    // Quiet NaN input: propagate canonical NaN, no exception.
    if x.is_nan() {
        return (CANON_QNAN_S, Flags::NONE);
    }
    // +/-0 -> +/-0 (sign preserved), exact.
    if x.is_zero() {
        return (a, Flags::NONE);
    }
    // Negative (finite or -inf): Invalid.
    if x.is_negative() {
        return (
            CANON_QNAN_S,
            Flags {
                invalid: true,
                ..Flags::NONE
            },
        );
    }
    // +inf -> +inf, exact.
    if x.is_infinite() {
        return (a, Flags::NONE);
    }

    // Positive finite. Host f64 sqrt of the exact f64 value is correctly rounded
    // to f64 (IEEE), giving a seed comfortably within 1 ulp of the true single-
    // precision root. Round the seed into Single (nearest), then ULP-correct
    // using the exact residual so the result is correctly rounded in `fcsr_rm`.
    let xf = f32::from_bits(a) as f64;
    let seed_f = xf.sqrt() as f32; // f64->f32 nearest; within 1 ulp of true root
    let seed = Single::from_bits(seed_f.to_bits() as u128);
    let (result, inexact) = correctly_round_sqrt_s(seed, x, fcsr_rm);
    (
        canon_s(result),
        Flags {
            inexact,
            ..Flags::NONE
        },
    )
}

/// Given a within-1-ulp `seed` for `sqrt(x)` (x positive finite), return the
/// correctly rounded `Single` for `fcsr_rm` and whether the op is inexact.
fn correctly_round_sqrt_s(seed: Single, x: Single, fcsr_rm: u8) -> (Single, bool) {
    use core::cmp::Ordering::{Equal, Greater, Less};
    // Snap the seed to the nearest of {seed, seed±1ulp} by residual, so `lo` is
    // the largest F-value with lo*lo <= x and `hi` the smallest with hi*hi >= x.
    let (lo, hi) = bracket_sqrt_s(seed, x);
    if lo.bitwise_eq(hi) {
        // seed*seed == x exactly: the root is representable, result is exact.
        return (lo, false);
    }
    // lo*lo < x < hi*hi, and hi == nextUp(lo). Pick per rounding mode.
    let rounded = match round_of(fcsr_rm) {
        Round::TowardZero | Round::TowardNegative => lo, // x>=0 so toward-zero == toward-neg
        Round::TowardPositive => hi,
        Round::NearestTiesToEven => {
            // No midpoint exists for sqrt, so compare true root to the midpoint
            // of [lo,hi] via residual against ((lo+hi)/2)^2 is unnecessary:
            // compare which of lo,hi is nearer by the exact residuals' magnitudes.
            match nearest_of_bracket_s(lo, hi, x) {
                Less => lo,
                Greater => hi,
                Equal => unreachable!("sqrt has no round-to-nearest midpoint"),
            }
        }
        Round::NearestTiesToAway => match nearest_of_bracket_s(lo, hi, x) {
            Less => lo,
            _ => hi,
        },
    };
    (rounded, true)
}

/// Return `(lo, hi)` bracketing the true root: the largest F-value with
/// `lo*lo <= x` and the smallest with `hi*hi >= x`, given a within-1-ulp seed.
/// If the root is exact, `lo == hi == the exact root`.
fn bracket_sqrt_s(seed: Single, x: Single) -> (Single, Single) {
    use core::cmp::Ordering::{Equal, Greater, Less};
    match residual_cmp_s(seed, x) {
        Equal => (seed, seed),
        Less => {
            // seed*seed < x: seed is lo (or below). Step up until hi*hi >= x.
            let up = seed.next_up().value;
            match residual_cmp_s(up, x) {
                Less => {
                    // Seed was >1 ulp low (shouldn't happen, but stay correct).
                    let up2 = up.next_up().value;
                    (up, up2)
                }
                Equal => (up, up),
                Greater => (seed, up),
            }
        }
        Greater => {
            // seed*seed > x: seed is hi (or above). Step down until lo*lo <= x.
            let down = seed.next_down().value;
            match residual_cmp_s(down, x) {
                Greater => {
                    let down2 = down.next_down().value;
                    (down2, down)
                }
                Equal => (down, down),
                Less => (down, seed),
            }
        }
    }
}

/// Decide whether the true root is nearer `lo` or `hi` using exact residuals.
/// The true root r satisfies lo < r < hi. r is nearer lo iff r < (lo+hi)/2 iff
/// x = r^2 < ((lo+hi)/2)^2. We compute ((lo+hi)/2)^2 exactly in the wider format
/// and compare to x. Returns `Less` if nearer lo, `Greater` if nearer hi.
fn nearest_of_bracket_s(lo: Single, hi: Single, x: Single) -> core::cmp::Ordering {
    let lo_w = widen_s_to_d(lo);
    let hi_w = widen_s_to_d(hi);
    let x_w = widen_s_to_d(x);
    // mid = (lo+hi)/2 — exact in Double (lo,hi are adjacent Singles; their sum
    // has <=25 bits and the /2 is an exponent shift).
    let sum = lo_w.add_r(hi_w, Round::NearestTiesToEven).value;
    let mid = sum
        .div_r(
            Double::from_bits((2.0f64).to_bits() as u128),
            Round::NearestTiesToEven,
        )
        .value;
    // mid^2: mid has <=25 significant bits, square <=50 < 53, exact in Double.
    let mid_sq = mid.mul_r(mid, Round::NearestTiesToEven).value;
    x_w.partial_cmp(&mid_sq).expect("finite operands compare")
}

/// `fd = sqrt(fs)` (double), correctly rounded under FCSR.RM.
///
/// Same structure as [`sqrt_s`]; the exact residual is evaluated in a wider
/// significand space via [`SqrtWide`] because no native type is wide enough.
pub fn sqrt_d(a: u64, fcsr_rm: u8) -> (u64, Flags) {
    let x = Double::from_bits(a as u128);
    if is_snan_d(a) {
        return (
            CANON_QNAN_D,
            Flags {
                invalid: true,
                ..Flags::NONE
            },
        );
    }
    if x.is_nan() {
        return (CANON_QNAN_D, Flags::NONE);
    }
    if x.is_zero() {
        return (a, Flags::NONE);
    }
    if x.is_negative() {
        return (
            CANON_QNAN_D,
            Flags {
                invalid: true,
                ..Flags::NONE
            },
        );
    }
    if x.is_infinite() {
        return (a, Flags::NONE);
    }

    // Positive finite. f64::sqrt is IEEE correctly-rounded to f64 in RN, so it is
    // already the correct answer for RN and within 1 ulp for the directed modes.
    let xf = f64::from_bits(a);
    let seed_f = xf.sqrt();
    let seed = Double::from_bits(seed_f.to_bits() as u128);
    let (result, inexact) = correctly_round_sqrt_d(seed, x, fcsr_rm);
    (
        canon_d(result),
        Flags {
            inexact,
            ..Flags::NONE
        },
    )
}

/// Exact residual sign of `y*y - x` for doubles, computed with a 128-bit-wide
/// significand ([`Quad`] holds `y*y`'s ≤106 bits and `x` exactly).
fn residual_cmp_d(y: Double, x: Double) -> core::cmp::Ordering {
    let y_wide = widen_d_to_q(y);
    let x_wide = widen_d_to_q(x);
    let prod = y_wide.mul_r(y_wide, Round::NearestTiesToEven).value;
    prod.partial_cmp(&x_wide).expect("finite operands compare")
}

/// Losslessly promote a finite `Double` to `Quad` (128-bit) via apfloat's exact
/// widening convert.
fn widen_d_to_q(v: Double) -> rustc_apfloat::ieee::Quad {
    use rustc_apfloat::ieee::Quad;
    use rustc_apfloat::FloatConvert;
    let mut loses_info = false;
    let q: Quad = v.convert(&mut loses_info).value;
    debug_assert!(!loses_info, "Double widens into Quad exactly");
    q
}

fn correctly_round_sqrt_d(seed: Double, x: Double, fcsr_rm: u8) -> (Double, bool) {
    use core::cmp::Ordering::{Equal, Greater, Less};
    let (lo, hi) = bracket_sqrt_d(seed, x);
    if lo.bitwise_eq(hi) {
        return (lo, false);
    }
    let rounded = match round_of(fcsr_rm) {
        Round::TowardZero | Round::TowardNegative => lo,
        Round::TowardPositive => hi,
        Round::NearestTiesToEven => match nearest_of_bracket_d(lo, hi, x) {
            Less => lo,
            Greater => hi,
            Equal => unreachable!("sqrt has no round-to-nearest midpoint"),
        },
        Round::NearestTiesToAway => match nearest_of_bracket_d(lo, hi, x) {
            Less => lo,
            _ => hi,
        },
    };
    (rounded, true)
}

fn bracket_sqrt_d(seed: Double, x: Double) -> (Double, Double) {
    use core::cmp::Ordering::{Equal, Greater, Less};
    match residual_cmp_d(seed, x) {
        Equal => (seed, seed),
        Less => {
            let up = seed.next_up().value;
            match residual_cmp_d(up, x) {
                Less => {
                    let up2 = up.next_up().value;
                    (up, up2)
                }
                Equal => (up, up),
                Greater => (seed, up),
            }
        }
        Greater => {
            let down = seed.next_down().value;
            match residual_cmp_d(down, x) {
                Greater => {
                    let down2 = down.next_down().value;
                    (down2, down)
                }
                Equal => (down, down),
                Less => (down, seed),
            }
        }
    }
}

fn nearest_of_bracket_d(lo: Double, hi: Double, x: Double) -> core::cmp::Ordering {
    let lo_w = widen_d_to_q(lo);
    let hi_w = widen_d_to_q(hi);
    let x_w = widen_d_to_q(x);
    let sum = lo_w.add_r(hi_w, Round::NearestTiesToEven).value;
    let two = widen_d_to_q(Double::from_bits((2.0f64).to_bits() as u128));
    let mid = sum.div_r(two, Round::NearestTiesToEven).value;
    let mid_sq = mid.mul_r(mid, Round::NearestTiesToEven).value;
    x_w.partial_cmp(&mid_sq).expect("finite operands compare")
}

// ---------------------------------------------------------------------------
// abs / neg.
//
// ABS.fmt and NEG.fmt are NOT arithmetic on the VR4300: they are sign-bit
// operations that never round and raise no IEEE exception — EXCEPT that, like
// the quiet-compare path, an SNaN operand signals Invalid (and the result is the
// canonical quiet NaN). A quiet-NaN operand passes through canonicalized with
// the sign flipped/cleared as appropriate; no exception.
// ---------------------------------------------------------------------------

/// `fd = |fs|` (single): clear the sign bit. SNaN operand → Invalid + canonical
/// NaN; other NaN → canonical NaN (sign clear), no exception.
pub fn abs_s(a: u32) -> (u32, Flags) {
    if is_snan_s(a) {
        return (
            CANON_QNAN_S,
            Flags {
                invalid: true,
                ..Flags::NONE
            },
        );
    }
    if is_nan_bits_s(a) {
        return (CANON_QNAN_S, Flags::NONE);
    }
    (a & 0x7FFF_FFFF, Flags::NONE)
}

/// `fd = -fs` (single): flip the sign bit. SNaN handling as [`abs_s`].
pub fn neg_s(a: u32) -> (u32, Flags) {
    if is_snan_s(a) {
        return (
            CANON_QNAN_S,
            Flags {
                invalid: true,
                ..Flags::NONE
            },
        );
    }
    if is_nan_bits_s(a) {
        return (CANON_QNAN_S, Flags::NONE);
    }
    (a ^ 0x8000_0000, Flags::NONE)
}

/// `fd = |fs|` (double).
pub fn abs_d(a: u64) -> (u64, Flags) {
    if is_snan_d(a) {
        return (
            CANON_QNAN_D,
            Flags {
                invalid: true,
                ..Flags::NONE
            },
        );
    }
    if is_nan_bits_d(a) {
        return (CANON_QNAN_D, Flags::NONE);
    }
    (a & 0x7FFF_FFFF_FFFF_FFFF, Flags::NONE)
}

/// `fd = -fs` (double).
pub fn neg_d(a: u64) -> (u64, Flags) {
    if is_snan_d(a) {
        return (
            CANON_QNAN_D,
            Flags {
                invalid: true,
                ..Flags::NONE
            },
        );
    }
    if is_nan_bits_d(a) {
        return (CANON_QNAN_D, Flags::NONE);
    }
    (a ^ 0x8000_0000_0000_0000, Flags::NONE)
}

/// Any single-precision NaN (quiet or signaling): exponent all ones, nonzero
/// mantissa.
fn is_nan_bits_s(a: u32) -> bool {
    a & 0x7F80_0000 == 0x7F80_0000 && a & 0x007F_FFFF != 0
}

/// Any double-precision NaN.
fn is_nan_bits_d(a: u64) -> bool {
    a & 0x7FF0_0000_0000_0000 == 0x7FF0_0000_0000_0000 && a & 0x000F_FFFF_FFFF_FFFF != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    // RN=0, RZ=1, RP=2, RM=3.

    #[test]
    fn rm_changes_inexact_result_single() {
        // 1.0 / 3.0 is inexact; each mode gives a distinct last bit.
        let one = 1.0f32.to_bits();
        let three = 3.0f32.to_bits();
        let (rn, _) = div_s(one, three, 0);
        let (rz, _) = div_s(one, three, 1);
        let (rp, _) = div_s(one, three, 2);
        let (rm, _) = div_s(one, three, 3);
        // 1/3 = 0.0101...; the true value falls between two representable f32s.
        // For a positive quotient: RZ and RM both take the lower neighbor; RP
        // takes the upper. RN here rounds to the upper neighbor (next bit is 1),
        // so RN == RP == upper and RZ == RM == lower — proving the mode is
        // honored (RZ/RM differ from RN/RP by exactly one ULP).
        assert_eq!(rz, rm, "positive operand: RZ == RM (both toward the lower)");
        assert_eq!(rn, rp, "1/3 RN rounds up, matching RP (upper neighbor)");
        assert_eq!(
            rp,
            rz + 1,
            "the upper neighbor is exactly one ULP above lower"
        );
        assert_ne!(
            rn, rz,
            "RN and RZ produce different bits on this inexact op"
        );
        // Host RN must still match (regression guard on the common case).
        assert_eq!(
            rn,
            (1.0f32 / 3.0f32).to_bits(),
            "RN matches the host result"
        );
    }

    #[test]
    fn rn_matches_host_for_ordinary_ops() {
        // In-range round-to-nearest ops must match the host float result bit for
        // bit (regression: the common case is unchanged).
        for &(a, b) in &[(1.5f32, 2.25f32), (3.0, 7.0), (-4.5, 0.75), (100.0, 0.1)] {
            let (r, f) = add_s(a.to_bits(), b.to_bits(), 0);
            assert_eq!(r, (a + b).to_bits(), "add {a}+{b}");
            let _ = f;
            let (r, _) = mul_s(a.to_bits(), b.to_bits(), 0);
            assert_eq!(r, (a * b).to_bits(), "mul {a}*{b}");
        }
    }

    #[test]
    fn div_by_zero_sets_z_single() {
        let (r, f) = div_s(1.0f32.to_bits(), 0.0f32.to_bits(), 0);
        assert!(f.divbyzero, "1/0 sets DivByZero");
        assert_eq!(r, f32::INFINITY.to_bits());
    }

    #[test]
    fn sqrt_negative_sets_v_single() {
        let (r, f) = sqrt_s((-1.0f32).to_bits(), 0);
        assert!(f.invalid, "sqrt(-1) sets Invalid");
        assert_eq!(r, CANON_QNAN_S, "sqrt(-1) is the canonical NaN");
    }

    #[test]
    fn overflow_sets_o_single() {
        let big = f32::MAX;
        let (_r, f) = mul_s(big.to_bits(), big.to_bits(), 0);
        assert!(f.overflow, "MAX*MAX overflows");
        assert!(f.inexact, "an overflow is also inexact");
    }

    #[test]
    fn inexact_sets_i_single() {
        let (_r, f) = div_s(1.0f32.to_bits(), 3.0f32.to_bits(), 0);
        assert!(f.inexact, "1/3 is inexact");
    }

    #[test]
    fn sqrt_correctly_rounded_single_matches_host_rn() {
        // For a large sweep, RN sqrt must match the host (which is IEEE correctly
        // rounded for f32 via the f64 path).
        for i in 0..2000u32 {
            let x = (i as f32) * 0.5 + 0.25;
            let (r, _) = sqrt_s(x.to_bits(), 0);
            assert_eq!(r, x.sqrt().to_bits(), "sqrt({x}) RN");
        }
    }

    #[test]
    fn sqrt_perfect_square_is_exact_single() {
        for n in 1u32..64 {
            let sq = (n * n) as f32;
            let (r, f) = sqrt_s(sq.to_bits(), 0);
            assert_eq!(r, (n as f32).to_bits(), "sqrt({sq}) exact");
            assert!(!f.inexact, "perfect square is exact");
        }
    }

    #[test]
    fn sqrt_rm_directed_brackets_true_root_single() {
        // For an inexact root, RZ result squared <= x <= RP result squared, and
        // RP == nextUp(RZ).
        let x = 2.0f32;
        let (rz, _) = sqrt_s(x.to_bits(), 1);
        let (rp, _) = sqrt_s(x.to_bits(), 2);
        let rz_f = f32::from_bits(rz);
        let rp_f = f32::from_bits(rp);
        assert!((rz_f as f64) * (rz_f as f64) <= x as f64, "RZ below root");
        assert!((rp_f as f64) * (rp_f as f64) >= x as f64, "RP above root");
        assert_eq!(rp_f.to_bits(), (rz_f).to_bits() + 1, "RP == nextUp(RZ)");
    }

    #[test]
    fn rm_changes_inexact_result_double() {
        let one = 1.0f64.to_bits();
        let three = 3.0f64.to_bits();
        let (rn, _) = div_d(one, three, 0);
        let (rz, _) = div_d(one, three, 1);
        let (rp, _) = div_d(one, three, 2);
        assert_ne!(rn, rp, "RN != RP on 1/3 double");
        assert_eq!(rz, div_d(one, three, 3).0, "positive: RZ == RM");
    }

    #[test]
    fn rn_matches_host_double() {
        for &(a, b) in &[(1.5f64, 2.25f64), (3.0, 7.0), (-4.5, 0.75)] {
            let (r, _) = add_d(a.to_bits(), b.to_bits(), 0);
            assert_eq!(r, (a + b).to_bits());
            let (r, _) = mul_d(a.to_bits(), b.to_bits(), 0);
            assert_eq!(r, (a * b).to_bits());
        }
    }

    #[test]
    fn div_by_zero_sets_z_double() {
        let (r, f) = div_d(1.0f64.to_bits(), 0.0f64.to_bits(), 0);
        assert!(f.divbyzero);
        assert_eq!(r, f64::INFINITY.to_bits());
    }

    #[test]
    fn sqrt_negative_sets_v_double() {
        let (r, f) = sqrt_d((-2.0f64).to_bits(), 0);
        assert!(f.invalid);
        assert_eq!(r, CANON_QNAN_D);
    }

    #[test]
    fn sqrt_correctly_rounded_double_matches_host_rn() {
        for i in 0..2000u64 {
            let x = (i as f64) * 0.5 + 0.25;
            let (r, _) = sqrt_d(x.to_bits(), 0);
            assert_eq!(r, x.sqrt().to_bits(), "sqrt({x}) RN double");
        }
    }

    #[test]
    fn snan_operand_sets_invalid_and_canonicalizes() {
        // Single SNaN (legacy MIPS: exponent all 1s, mantissa MSB clear, nonzero).
        let snan = 0x7FA0_0000u32;
        assert!(is_snan_s(snan));
        let (r, f) = add_s(snan, 1.0f32.to_bits(), 0);
        assert!(f.invalid, "SNaN operand -> Invalid");
        assert_eq!(r, CANON_QNAN_S, "result is canonical quiet NaN");
    }

    #[test]
    fn quiet_nan_result_is_canonical_single() {
        // inf - inf is Invalid and yields the canonical NaN (not apfloat's 2008
        // encoding).
        let (r, f) = sub_s(f32::INFINITY.to_bits(), f32::INFINITY.to_bits(), 0);
        assert!(f.invalid, "inf - inf is Invalid");
        assert_eq!(r, CANON_QNAN_S, "canonical NaN bits");
    }

    #[test]
    fn abs_neg_are_sign_ops_no_exception() {
        let (r, f) = abs_s((-3.5f32).to_bits());
        assert_eq!(r, 3.5f32.to_bits());
        assert_eq!(f, Flags::default());
        let (r, f) = neg_s(3.5f32.to_bits());
        assert_eq!(r, (-3.5f32).to_bits());
        assert_eq!(f, Flags::default());
        // abs/neg of an SNaN signals Invalid.
        let (r, f) = neg_s(0x7FA0_0000);
        assert!(f.invalid);
        assert_eq!(r, CANON_QNAN_S);
    }
}
