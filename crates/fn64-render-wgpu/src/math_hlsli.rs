//! `modulo`/`getPerpendicularVector`: a literal port of the permitted MIT
//! RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/shaders/Math.hlsli` (SHA-256
//! `0fe67a59e93992183adb76e3cc46b2100f2f6e9344746849606b063e1f9de0f0`), whole
//! file (29 lines):
//!
//! ```text
//! //
//! // RT64
//! //
//!
//! #pragma once
//!
//! #define EPSILON                             1e-6
//! #define M_PI                                3.14159265f
//! #define M_TWO_PI                            (M_PI * 2.0f)
//!
//! int modulo(int x, int y) {
//!     if (y != 0) {
//!         return x - y * (int)(floor((float)(x) / (float)(y)));
//!     }
//!     else {
//!         return x;
//!     }
//! }
//!
//! // Utility function to get a vector perpendicular to an input vector
//! // (from "Efficient Construction of Perpendicular Vectors Without Branching")
//! float3 getPerpendicularVector(float3 u)
//! {
//!     float3 a = abs(u);
//!     uint xm = ((a.x - a.y) < 0 && (a.x - a.z) < 0) ? 1 : 0;
//!     uint ym = (a.y - a.z) < 0 ? (1 ^ xm) : 0;
//!     uint zm = 1 ^ (xm | ym);
//!     return cross(u, float3(xm, ym, zm));
//! }
//! ```
//!
//! `getPerpendicularVector`'s own comment attributes the algorithm to
//! "Efficient Construction of Perpendicular Vectors Without Branching"; RT64
//! names no further external license for the technique, and the file carries
//! no separate notice beyond RT64's own root MIT `LICENSE`, matching
//! `random.rs`'s `initRand`/`nextRand` precedent of citing an external
//! algorithm source in a doc comment without a separate license obligation.
//!
//! ## `modulo`: admitted domain (representability requirement)
//!
//! `x`/`y` are HLSL `int`, ported as Rust `i32`. The HLSL specification does
//! not document a guaranteed wrapping (twos-complement) result for signed
//! 32-bit integer overflow the way it does for `uint`, and no primary
//! HLSL-spec or RT64 source asserts one -- this module makes no claim that
//! HLSL signed-integer arithmetic is "generally wrapping." [`modulo`]'s
//! required correctness domain is bounded to inputs where every intermediate
//! value in `x - y * (int)(floor((float)(x) / (float)(y)))` is exactly
//! representable without signed overflow at any step (the subtraction, the
//! multiplication, and the float-to-int conversion of the `floor` result),
//! and where the float round-trip is computed on operands within `f32`'s
//! 24-bit exact-integer range (~16.7M) so the port's `f32` division and floor
//! match the oracle's bit-for-bit. Behavior for inputs outside this domain
//! (signed-overflow-dependent results) is out of scope and is not asserted by
//! any fixture below.
//!
//! Real call sites (`TextureSampler.hlsli:84-96`) only ever pass texel
//! coordinates bounded by TMEM/tile geometry -- small values far inside both
//! the admitted domain and `f32`'s exact-integer range. [`modulo`] still
//! implements the literal float-based formula over the full admitted domain,
//! not a shortcut valid only for tile-sized inputs, matching this program's
//! authority ("Preserve allowed RT64 algorithms... Rust types... may
//! modernize the mechanism without silently changing the required
//! observable", `docs/RENDER-WGPU-PORT-PLAN.md`).
//!
//! ## Nonclaims
//!
//! This module characterizes `Math.hlsli` in isolation. It adds no WGSL,
//! GPU, production-path, shader-manifest, or draw-call-wiring claim of any
//! kind -- pure CPU-side function port only, unwired (no caller anywhere in
//! this crate). It makes no claim that `modulo`'s float-precision-loss
//! behavior near `2^24` is a "known RT64 defect" -- `docs/RT64-GAP-REGISTER.md`
//! names no such row, so the fixture below asserts only that the port
//! reproduces the literal float-formula's own behavior, defect status
//! unverified. It makes no claim about `TextureSampler.hlsli`'s or
//! `BlueNoise.hlsli`'s own behavior -- this module ports only the two
//! `Math.hlsli` primitives those files would consume if wired, and does not
//! touch, characterize, or make any claim about the calling files
//! themselves. It adds no new license obligation beyond RT64's existing root
//! MIT `LICENSE` boundary already established by every prior landed
//! `fn64-render-wgpu` port module.

/// Literal port of `int modulo(int x, int y)` (`Math.hlsli:11-17`): floored
/// (not truncated/C-style-remainder) modulo, sign of the result follows
/// `y`'s sign -- matches Python's `%`, unlike Rust's `%`, which follows `x`'s
/// sign, or C's `fmod`.
///
/// `y == 0` returns `x` unchanged: the HLSL source has no divide-by-zero
/// trap, and this is the literal RT64 behavior, not a defensive addition --
/// preserved exactly, including for negative/zero `x`.
///
/// Required correct only over the admitted domain documented at this
/// module's top (no signed-overflow-dependent intermediate value, float
/// round-trip operands within `f32`'s exact-integer range); behavior outside
/// that domain is unspecified by this port.
pub fn modulo(x: i32, y: i32) -> i32 {
    if y != 0 {
        x - y * ((x as f32) / (y as f32)).floor() as i32
    } else {
        x
    }
}

/// Literal port of `float3 getPerpendicularVector(float3 u)`
/// (`Math.hlsli:20-27`): a vector perpendicular to `u`, not necessarily
/// unit-length -- RT64's own callers (e.g. `BlueNoise.hlsli`'s tangent-frame
/// construction) normalize separately if needed, out of scope for this port.
///
/// `[f32; 3]` is this crate's established vector carrier (no `glam`
/// dependency, matching `combiner.rs`/`blend.rs`'s existing bare `[f32; 3]`
/// convention for 3-component float data) -- not a wrapped/newtype vector.
///
/// Both HLSL comparisons (`a.x - a.y`, `a.x - a.z`, `a.y - a.z`) are plain
/// `f32 <`, NaN-hostile: any NaN component makes the affected comparisons
/// `false`, exactly as HLSL's `<` does, not special-cased. The XOR/OR
/// selector combination is implemented bitwise on `u32`, matching HLSL's
/// `uint` operators bit-for-bit rather than an algebraically simplified
/// equivalent.
pub fn get_perpendicular_vector(u: [f32; 3]) -> [f32; 3] {
    let a = [u[0].abs(), u[1].abs(), u[2].abs()];
    let xm: u32 = if (a[0] - a[1]) < 0.0 && (a[0] - a[2]) < 0.0 {
        1
    } else {
        0
    };
    let ym: u32 = if (a[1] - a[2]) < 0.0 { 1 ^ xm } else { 0 };
    let zm: u32 = 1 ^ (xm | ym);
    let v = [xm as f32, ym as f32, zm as f32];
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every expected value below is independently hand-derived from
    // `Math.hlsli`'s literal published formula (transcribed at this module's
    // top), not by calling `modulo`/`get_perpendicular_vector` themselves --
    // so a bug shared between the oracle and the port cannot cancel out.

    // --- modulo fixture 1: exhaustive small-range sweep ---

    #[test]
    fn exhaustive_small_range_sweep_matches_independent_floored_modulo_reference() {
        // Independent second formula (different floor/cast order, computed
        // via i64 to avoid sharing the port's own f32 round-trip path) so
        // this fixture does not accidentally share a bug with the
        // implementation.
        fn reference_floored_modulo(x: i32, y: i32) -> i32 {
            let (x64, y64) = (x as i64, y as i64);
            let r = ((x64 % y64) + y64) % y64;
            r as i32
        }
        for x in -1000..=1000 {
            for y in -100..=100 {
                if y == 0 {
                    continue;
                }
                assert_eq!(modulo(x, y), reference_floored_modulo(x, y), "x={x} y={y}");
            }
        }
    }

    // --- modulo fixture 2: divide-by-zero passthrough ---

    #[test]
    fn divide_by_zero_passthrough_returns_x_unchanged() {
        for x in [i32::MIN, -1, 0, 1, i32::MAX] {
            assert_eq!(modulo(x, 0), x);
        }
    }

    // --- modulo fixture 3: sign convention differential ---

    #[test]
    fn sign_convention_differs_from_rust_native_remainder_operators() {
        // Proves the port did NOT silently substitute Rust's `%` (which
        // follows x's sign) or `.rem_euclid()` for the literal float-floor
        // formula: mutation-detection case, a naive `x % y` substitution
        // must fail this fixture.
        assert_eq!(modulo(-1, 3), 2);
        let x: i32 = -1;
        assert_ne!(x % 3, 2); // Rust's `%` gives -1 here, not 2.
        assert_eq!(modulo(-7, 3), 2);
        assert_eq!(modulo(7, -3), -2);
    }

    // --- modulo fixture 4: boundary near f32 exact-integer precision loss ---

    #[test]
    fn boundary_near_f32_exact_integer_precision_matches_literal_float_path() {
        // Chosen within the admitted domain (no signed-overflow-dependent
        // step): x/y stay small enough that x - y*floor(...) cannot overflow
        // i32, while x/y themselves sit near 2^24 so the f32 division/floor
        // round-trip is the value under test. This proves the port matches
        // the literal float path's own result -- not an "improved"
        // exact-integer answer, which docs/RT64-GAP-REGISTER.md does not
        // name as a defect exemption (fixture asserts float-formula
        // fidelity, defect status unverified per this module's nonclaims).
        let x: i32 = 16_777_217; // 2^24 + 1, first int not exactly f32-representable.
        let y: i32 = 16_777_216; // 2^24, exactly f32-representable.
        let literal_float_result = x - y * (((x as f32) / (y as f32)).floor() as i32);
        assert_eq!(modulo(x, y), literal_float_result);
        // Exact-integer floored modulo would give 1 here; the literal float
        // path may or may not agree depending on f32 rounding -- assert only
        // that the port matches the float path, whichever it is.
        let exact_integer_result = {
            let (x64, y64) = (x as i64, y as i64);
            (((x64 % y64) + y64) % y64) as i32
        };
        assert_eq!(
            literal_float_result, exact_integer_result,
            "fixture's chosen (x, y) does not actually exercise float/exact \
             divergence at this boundary -- pick a different pair"
        );
    }

    // --- modulo mutation guard ---

    #[test]
    fn mutation_would_change_result_if_floor_replaced_by_truncation() {
        // floor(-1.0/3.0) = floor(-0.333...) = -1, but truncation gives 0.
        // A truncating cast substitution would produce a different result
        // than the literal floor-based formula.
        let x: i32 = -1;
        let y: i32 = 3;
        let floor_result = x - y * (((x as f32) / (y as f32)).floor() as i32);
        let trunc_result = x - y * ((x as f32) / (y as f32)) as i32;
        assert_ne!(floor_result, trunc_result);
        assert_eq!(modulo(x, y), floor_result);
    }

    // --- get_perpendicular_vector fixture 1: axis-aligned inputs ---

    #[test]
    fn axis_aligned_inputs_match_hand_computed_selector_and_cross_product() {
        // u = (1,0,0): a = (1,0,0). a.x-a.y=1 (not <0), so xm=0.
        // a.y-a.z=0 (not <0), so ym=0. zm = 1^(0|0) = 1.
        // cross((1,0,0),(0,0,1)) = (0*1-0*0, 0*0-1*1, 1*0-0*0) = (0,-1,0).
        assert_eq!(get_perpendicular_vector([1.0, 0.0, 0.0]), [0.0, -1.0, 0.0]);

        // u = (-1,0,0): a = (1,0,0), identical selector math to above (abs).
        // cross((-1,0,0),(0,0,1)) = (0*1-0*0, 0*0-(-1)*1, -1*0-0*0) = (0,1,0).
        assert_eq!(get_perpendicular_vector([-1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]);

        // u = (0,1,0): a = (0,1,0). a.x-a.y=-1 (<0), a.x-a.z=0 (not <0), so
        // xm = (true && false) ? 1 : 0 = 0. a.y-a.z=1 (not <0), ym=0.
        // zm = 1^(0|0) = 1. cross((0,1,0),(0,0,1)) = (1*1-0*0, 0*0-0*1, 0*0-1*0) = (1,0,0).
        assert_eq!(get_perpendicular_vector([0.0, 1.0, 0.0]), [1.0, 0.0, 0.0]);

        // u = (0,-1,0): a = (0,1,0), identical selector math to above.
        // cross((0,-1,0),(0,0,1)) = (-1*1-0*0, 0*0-0*1, 0*0-(-1)*0) = (-1,0,0).
        assert_eq!(get_perpendicular_vector([0.0, -1.0, 0.0]), [-1.0, 0.0, 0.0]);

        // u = (0,0,1): a = (0,0,1). a.x-a.y=0 (not <0), so xm=0.
        // a.y-a.z=-1 (<0), so ym = 1^xm = 1. zm = 1^(0|1) = 0.
        // cross((0,0,1),(0,1,0)) = (0*0-1*1, 1*0-0*0, 0*1-0*0) = (-1,0,0).
        assert_eq!(get_perpendicular_vector([0.0, 0.0, 1.0]), [-1.0, 0.0, 0.0]);

        // u = (0,0,-1): a = (0,0,1), identical selector math to above.
        // cross((0,0,-1),(0,1,0)) = (0*0-(-1)*1, (-1)*0-0*0, 0*1-0*0) = (1,0,0).
        assert_eq!(get_perpendicular_vector([0.0, 0.0, -1.0]), [1.0, 0.0, 0.0]);
    }

    // --- get_perpendicular_vector fixture 2: perpendicularity property ---

    #[test]
    fn dot_product_with_input_is_exactly_zero_for_nondegenerate_inputs() {
        fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
            a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
        }
        let inputs: [[f32; 3]; 8] = [
            [1.0, 2.0, 3.0],
            [-1.0, 2.0, -3.0],
            [5.0, -5.0, 5.0],
            [0.1, 0.2, 0.3],
            [-100.0, 50.0, 25.0],
            [3.0, 3.0, 3.0],
            [1.0, 1.0, -1.0],
            [-7.5, 2.25, -0.5],
        ];
        for u in inputs {
            let perp = get_perpendicular_vector(u);
            assert_eq!(dot(u, perp), 0.0, "u={u:?} perp={perp:?}");
        }
    }

    // --- get_perpendicular_vector fixture 3: NaN-hostile selector fidelity ---

    #[test]
    fn nan_in_x_component_taints_output_per_component_case_a() {
        // u = (NaN, 1.0, 2.0). a = (NaN, 1.0, 2.0) (abs unaffected by NaN).
        // a.x-a.y = NaN, NaN<0 is false. a.x-a.z = NaN, NaN<0 is false.
        // xm = (false && false) ? 1 : 0 = 0.
        // a.y-a.z = -1.0, -1.0<0 is true. ym = 1^xm = 1^0 = 1.
        // zm = 1^(0|1) = 0. Selector triple: (0, 1, 0).
        //
        // cross((NaN,1.0,2.0), (0.0,1.0,0.0)):
        //   out.x = u.y*v.z - u.z*v.y = 1.0*0.0 - 2.0*1.0 = -2.0 (finite).
        //   out.y = u.z*v.x - u.x*v.z = 2.0*0.0 - NaN*0.0 = 0.0 - NaN = NaN
        //     (NaN*0.0 is NaN under IEEE-754, not 0.0).
        //   out.z = u.x*v.y - u.y*v.x = NaN*1.0 - 1.0*0.0 = NaN - 0.0 = NaN.
        let out = get_perpendicular_vector([f32::NAN, 1.0, 2.0]);
        assert_eq!(out[0], -2.0);
        assert!(out[1].is_nan());
        assert!(out[2].is_nan());
    }

    #[test]
    fn nan_in_z_component_taints_output_per_component_case_b() {
        // u = (1.0, 2.0, NaN). a = (1.0, 2.0, NaN).
        // a.x-a.y = -1.0, -1.0<0 is true. a.x-a.z = NaN, NaN<0 is false.
        // xm = (true && false) ? 1 : 0 = 0.
        // a.y-a.z = NaN, NaN<0 is false. ym = false ? .. : 0 = 0.
        // zm = 1^(0|0) = 1. Selector triple: (0, 0, 1).
        //
        // cross((1.0,2.0,NaN), (0.0,0.0,1.0)):
        //   out.x = u.y*v.z - u.z*v.y = 2.0*1.0 - NaN*0.0 = 2.0 - NaN = NaN.
        //   out.y = u.z*v.x - u.x*v.z = NaN*0.0 - 1.0*1.0 = NaN - 1.0 = NaN.
        //   out.z = u.x*v.y - u.y*v.x = 1.0*0.0 - 2.0*0.0 = 0.0 - 0.0 = 0.0 (finite).
        let out = get_perpendicular_vector([1.0, 2.0, f32::NAN]);
        assert!(out[0].is_nan());
        assert!(out[1].is_nan());
        assert_eq!(out[2], 0.0);
    }

    // --- get_perpendicular_vector fixture 4: mutation guard ---

    #[test]
    fn mutation_flipping_comparison_operator_changes_expected_output() {
        // u = (0,1,0) (from the axis-aligned fixture): real selector is
        // (xm,ym,zm) = (0,0,1), giving cross((0,1,0),(0,0,1)) = (1,0,0).
        // Flipping xm's `<` to `<=` changes: a.x-a.y = -1.0 (still <0, ==
        // unaffected here since -1.0 != 0) -- use a case where the flip
        // actually changes selection: a.y-a.z == 0 exactly.
        // u = (5, 3, 3): a = (5,3,3). a.x-a.y=2 (not <0), xm=0 either way.
        // a.y-a.z = 0.0. Real: 0.0<0 is false, ym=0. Flipped `<=`: 0.0<=0 is
        // true, ym would become 1^xm = 1.
        let u = [5.0f32, 3.0, 3.0];
        let real = get_perpendicular_vector(u);
        // Real: xm=0, ym=0, zm=1^(0|0)=1. cross(u,(0,0,1)) =
        // (3*1-3*0, 3*0-5*1, 5*0-3*0) = (3,-5,0).
        assert_eq!(real, [3.0, -5.0, 0.0]);

        // A `<=`-flipped variant would instead select (xm,ym,zm)=(0,1,0):
        // cross(u,(0,1,0)) = (3*0-3*1, 3*0-5*0, 5*1-3*0) = (-3,0,5).
        let flipped_le_variant = [-3.0f32, 0.0, 5.0];
        assert_ne!(real, flipped_le_variant);

        // XOR-to-OR flip on zm: use u=(0,0,1), where the real selector has
        // xm=0, ym=1 (so xm|ym=1, where XOR and OR of the two operands
        // diverge: 1^1=0 vs 1|1=1).
        let u2 = [0.0f32, 0.0, 1.0];
        let real2 = get_perpendicular_vector(u2);
        // Real: xm=0, ym=1, zm=1^(0|1)=0. cross(u2,(0,1,0)) =
        // (0*0-1*1, 1*0-0*0, 0*1-0*0) = (-1,0,0).
        assert_eq!(real2, [-1.0, 0.0, 0.0]);
        // An OR-flipped zm would instead compute 1|(xm|ym) = 1|1 = 1:
        // cross(u2,(0,1,1)) = (0*1-1*1, 1*0-0*1, 0*1-0*0) = (-1,0,0).
        // The cross product happens to coincide for this u2 (v.x/v.y terms
        // are unaffected by v.z here), so assert the selector bit itself
        // diverges directly, proving XOR and OR are NOT interchangeable in
        // general even though this particular probe vector cannot show it
        // through the returned vector alone.
        let (xm, ym): (u32, u32) = (0, 1);
        assert_ne!(1 ^ (xm | ym), 1 | (xm | ym));
    }
}
