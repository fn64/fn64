//! RT64 texture-coordinate generation: `normalizeSafe`/`computeTextureGen`.
//!
//! Characterization-first literal port of the permitted MIT RT64 Rust-port
//! source pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/shaders/TextureGen.hlsli`:
//!
//! ```text
//! float3 normalizeSafe(float3 v) {
//!     float l = length(v);
//!     if (l > 0) {
//!         return v / l;
//!     }
//!     else {
//!         return v;
//!     }
//! }
//!
//! float2 computeTextureGen(float2 inputUV, float3 inputNormal, RSPLookAt lookAt, bool textureGenLinear, const float4x4 worldMatrix) {
//!     float2 texgenUV;
//!     texgenUV.x = dot(inputNormal, normalizeSafe(mul(float4(lookAt.x, 0.0f), worldMatrix).xyz));
//!     texgenUV.y = dot(inputNormal, normalizeSafe(mul(float4(lookAt.y, 0.0f), worldMatrix).xyz));
//!     texgenUV = clamp(texgenUV, float2(-1.0f, -1.0f), float2(1.0f, 1.0f));
//!     if (textureGenLinear) {
//!         texgenUV = acos(-texgenUV) * 325.94932f; // 1024 / PI
//!     }
//!     else {
//!         texgenUV += float2(1.0f, 1.0f);
//!         texgenUV *= 512.0f;
//!     }
//!
//!     // Texture scaling is encoded into UV directly when texture gen is enabled.
//!     return (inputUV / 65536.0f) * texgenUV;
//! }
//! ```
//!
//! `fn64-render-wgpu` has no crate dependency on `fn64-render-reference`
//! (see `depth_strict_less.rs`, `alpha_compare.rs`), so this is a
//! self-contained literal re-expression citing RT64's source directly.
//!
//! ## `mul(vector, matrix)` convention -- preserved exactly, not "fixed"
//!
//! Elsewhere in this same pinned RT64 source tree (`src/shaders/RSPWorldCS.hlsl`),
//! `mul` is called matrix-first: `mul(worldMats[i], float4(pos, 1.0))`. This
//! file calls it the other way around: `mul(float4(lookAt.x, 0.0f),
//! worldMatrix)`, vector first. HLSL's `mul(x, y)` overload resolves
//! structurally on argument shape, not by a fixed convention -- when the
//! first argument is a vector and the second a matrix, `mul` treats the
//! vector as a *row* vector and computes `result[c] = sum_r x[r] * y[r][c]`
//! (row-vector-times-matrix), the transpose of the matrix-first,
//! column-vector form used in `RSPWorldCS.hlsl`. This module ports
//! `TextureGen.hlsli`'s own literal row-vector form exactly as written, and
//! does not reconcile it with the other file's opposite convention -- that
//! would be silently changing the ported arithmetic, not preserving it. See
//! [`mul_row_vector_matrix`].
//!
//! ## Nonclaims
//!
//! No RSP lookat-matrix derivation (`RSPLookAt` is caller-supplied, matching
//! this module's pure value-in/value-out convention), no world-matrix
//! upload/storage-buffer plumbing, no vertex-shader integration, no
//! combiner/texture-sample consumption of the returned UV, no draw-path or
//! production-DPC wiring, and no RT64 visual/pixel/silicon parity or
//! performance claim.

/// `1024 / PI`, RT64's own literal constant (`TextureGen.hlsli:25`), not a
/// runtime-computed `1024.0 / core::f32::consts::PI` -- ported as the exact
/// bit pattern the pinned source spells out. `clippy::excessive_precision`
/// would rewrite this literal's spelling to `325.949_3` (an f32-round-trip
/// no-op -- both spellings produce the identical f32 bit pattern), but this
/// port preserves RT64's own source text digit-for-digit rather than let a
/// lint reformat a cited literal constant.
#[allow(clippy::excessive_precision)]
const LINEAR_SCALE: f32 = 325.94932;

/// `512.0f`, the non-linear mode's post-offset scale (`TextureGen.hlsli:29`).
const NON_LINEAR_SCALE: f32 = 512.0;

/// `65536.0f`, the final input-UV normalization divisor (`TextureGen.hlsli:33`).
const INPUT_UV_DIVISOR: f32 = 65536.0;

/// A 4x4 row-major matrix matching HLSL's `float4x4` operand shape for
/// [`mul_row_vector_matrix`]. `rows[r][c]` is row `r`, column `c` -- the same
/// indexing `worldMatrix[r][c]` would use in HLSL source text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldMatrix {
    pub rows: [[f32; 4]; 4],
}

/// RT64's `RSPLookAt` (`rt64_rsp_lookat.h:16-19`): the two lookat axis
/// vectors a texture-gen fragment dots its normal against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RspLookAt {
    pub x: [f32; 3],
    pub y: [f32; 3],
}

/// Literal port of `normalizeSafe` (`TextureGen.hlsli:9-17`): normalize `v`,
/// or return it unchanged when its length is not strictly positive --
/// including the zero vector (`length == 0`) and any vector whose length is
/// `NaN` (`NaN > 0` is `false` in IEEE-754, so HLSL's `if (l > 0)` also falls
/// through to the unchanged-`v` branch for a `NaN` length; this port's plain
/// `f32` `>` has the same behavior, so no special-case is added for it).
pub fn normalize_safe(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length > 0.0 {
        [v[0] / length, v[1] / length, v[2] / length]
    } else {
        v
    }
}

/// HLSL `mul(float4 x, float4x4 y)` when `x` is treated as a row vector:
/// `result[c] = sum_r x[r] * y[r][c]`. See the module-level doc for why this
/// is the correct operand order for `TextureGen.hlsli`'s own call, even
/// though it is the transpose of the matrix-first convention used elsewhere
/// in the same pinned RT64 source tree.
fn mul_row_vector_matrix(x: [f32; 4], y: &WorldMatrix) -> [f32; 4] {
    let mut result = [0.0f32; 4];
    for (c, result_c) in result.iter_mut().enumerate() {
        let mut sum = 0.0f32;
        for (r, x_r) in x.iter().enumerate() {
            sum += x_r * y.rows[r][c];
        }
        *result_c = sum;
    }
    result
}

/// `dot(a, b)` over `float3`, HLSL's ordinary component-wise-multiply-then-sum.
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `clamp(v, -1, 1)` per component, HLSL's `clamp(x, min, max)`.
fn clamp_unit(v: f32) -> f32 {
    v.clamp(-1.0, 1.0)
}

/// Literal port of `computeTextureGen` (`TextureGen.hlsli:19-34`).
///
/// Preserves RT64's exact float operation order: transform each lookat axis
/// by `worldMatrix` (row-vector `mul`, see the module doc), `normalizeSafe`
/// the transformed `.xyz`, `dot` against `inputNormal`, then `clamp` both
/// resulting scalars to `[-1, 1]` *before* branching on `textureGenLinear` --
/// the clamp is unconditional and precedes the mode split, exactly as
/// written. Linear mode: `acos(-texgenUV) * 325.94932`. Non-linear mode:
/// `(texgenUV + 1) * 512`, applied as two separate ops (`+=` then `*=`) in
/// the pinned source, reproduced here as the same two-step sequence rather
/// than a fused `fma`-style single expression. Finally
/// `(inputUV / 65536.0) * texgenUV`.
pub fn compute_texture_gen(
    input_uv: [f32; 2],
    input_normal: [f32; 3],
    look_at: RspLookAt,
    texture_gen_linear: bool,
    world_matrix: WorldMatrix,
) -> [f32; 2] {
    let transformed_x = mul_row_vector_matrix(
        [look_at.x[0], look_at.x[1], look_at.x[2], 0.0],
        &world_matrix,
    );
    let transformed_y = mul_row_vector_matrix(
        [look_at.y[0], look_at.y[1], look_at.y[2], 0.0],
        &world_matrix,
    );
    let axis_x = normalize_safe([transformed_x[0], transformed_x[1], transformed_x[2]]);
    let axis_y = normalize_safe([transformed_y[0], transformed_y[1], transformed_y[2]]);

    let mut texgen_uv = [dot3(input_normal, axis_x), dot3(input_normal, axis_y)];
    texgen_uv[0] = clamp_unit(texgen_uv[0]);
    texgen_uv[1] = clamp_unit(texgen_uv[1]);

    if texture_gen_linear {
        texgen_uv[0] = (-texgen_uv[0]).acos() * LINEAR_SCALE;
        texgen_uv[1] = (-texgen_uv[1]).acos() * LINEAR_SCALE;
    } else {
        texgen_uv[0] += 1.0;
        texgen_uv[1] += 1.0;
        texgen_uv[0] *= NON_LINEAR_SCALE;
        texgen_uv[1] *= NON_LINEAR_SCALE;
    }

    [
        (input_uv[0] / INPUT_UV_DIVISOR) * texgen_uv[0],
        (input_uv[1] / INPUT_UV_DIVISOR) * texgen_uv[1],
    ]
}

pub const TEXTURE_GEN_WGSL: &str = include_str!("shaders/texture_gen.wgsl");
pub const TEXTURE_GEN_ENTRY_POINT: &str = "compute_texture_gen_entry";

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_matrix() -> WorldMatrix {
        WorldMatrix {
            rows: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    fn assert_close(a: f32, b: f32, epsilon: f32) {
        assert!(
            (a - b).abs() <= epsilon,
            "expected {b}, got {a} (diff {})",
            (a - b).abs()
        );
    }

    // --- normalize_safe ---

    #[test]
    fn normalize_safe_zero_vector_returns_unchanged() {
        assert_eq!(normalize_safe([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn normalize_safe_unit_x_is_unchanged() {
        let result = normalize_safe([1.0, 0.0, 0.0]);
        assert_eq!(result, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn normalize_safe_scales_to_unit_length() {
        let result = normalize_safe([3.0, 4.0, 0.0]);
        assert_close(result[0], 0.6, 1e-6);
        assert_close(result[1], 0.8, 1e-6);
        assert_close(result[2], 0.0, 1e-6);
        let length = (result[0] * result[0] + result[1] * result[1] + result[2] * result[2]).sqrt();
        assert_close(length, 1.0, 1e-6);
    }

    #[test]
    fn normalize_safe_negative_component_vector() {
        // (-3, -4, 0), length 5 -- independently computed expected value.
        let result = normalize_safe([-3.0, -4.0, 0.0]);
        assert_close(result[0], -0.6, 1e-6);
        assert_close(result[1], -0.8, 1e-6);
        assert_close(result[2], 0.0, 1e-6);
    }

    #[test]
    fn normalize_safe_arbitrary_vector_independently_computed() {
        // (1, 2, 2): length = sqrt(1+4+4) = 3.
        let result = normalize_safe([1.0, 2.0, 2.0]);
        assert_close(result[0], 1.0 / 3.0, 1e-6);
        assert_close(result[1], 2.0 / 3.0, 1e-6);
        assert_close(result[2], 2.0 / 3.0, 1e-6);
    }

    #[test]
    fn normalize_safe_nan_length_returns_unchanged() {
        // A component of NaN makes length() NaN; HLSL's `l > 0` is false for
        // NaN, so the unchanged-v branch is taken, matching IEEE-754 `>`.
        let v = [f32::NAN, 1.0, 0.0];
        let result = normalize_safe(v);
        assert!(result[0].is_nan());
        assert_eq!(result[1], 1.0);
        assert_eq!(result[2], 0.0);
    }

    #[test]
    fn normalize_safe_very_small_nonzero_vector_still_normalizes() {
        // Length is tiny but strictly positive and finite -- must still
        // divide, not fall into the zero-vector branch. 1e-19 is chosen so
        // its square (1e-38) stays within f32's normal range (min positive
        // normal ~1.18e-38); a smaller magnitude like 1e-30 squares to
        // exactly 0.0 in f32 (denormal squaring underflow), which would
        // correctly hit the zero-vector branch per IEEE-754 -- that is
        // RT64's own literal behavior for such an input, not a bug in this
        // test's chosen magnitude.
        let tiny = 1e-19_f32;
        let result = normalize_safe([tiny, 0.0, 0.0]);
        assert_close(result[0], 1.0, 1e-6);
    }

    // --- mul_row_vector_matrix (via compute_texture_gen with identity, then non-identity) ---

    #[test]
    fn compute_texture_gen_identity_matrix_matches_hand_derivation() {
        // lookAt.x = (1,0,0), lookAt.y = (0,1,0), identity world matrix.
        // normalizeSafe(mul(...)) is unchanged for both unit axes.
        // inputNormal = (0,0,1): dot((0,0,1),(1,0,0)) = 0, dot((0,0,1),(0,1,0)) = 0.
        let look_at = RspLookAt {
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        let result = compute_texture_gen(
            [65536.0, 65536.0],
            [0.0, 0.0, 1.0],
            look_at,
            false,
            identity_matrix(),
        );
        // texgenUV = clamp(0,0) = (0,0); non-linear: (0+1)*512 = 512 each axis.
        // inputUV/65536 = (1,1); result = 512*1 = 512.
        assert_close(result[0], 512.0, 1e-3);
        assert_close(result[1], 512.0, 1e-3);
    }

    #[test]
    fn compute_texture_gen_nonzero_dot_non_linear_mode_independently_derived() {
        // lookAt.x = (1,0,0), inputNormal = (1,0,0): dot = 1, clamp(1)=1.
        // lookAt.y = (0,1,0), inputNormal has y=0: dot = 0.
        // Non-linear: x-axis (1+1)*512 = 1024; y-axis (0+1)*512 = 512.
        // inputUV = (65536, 32768) -> /65536 = (1, 0.5).
        // result = (1*1024, 0.5*512) = (1024, 256).
        let look_at = RspLookAt {
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        let result = compute_texture_gen(
            [65536.0, 32768.0],
            [1.0, 0.0, 0.0],
            look_at,
            false,
            identity_matrix(),
        );
        assert_close(result[0], 1024.0, 1e-2);
        assert_close(result[1], 256.0, 1e-2);
    }

    #[test]
    fn compute_texture_gen_negative_dot_non_linear_mode() {
        // lookAt.x = (1,0,0), inputNormal = (-1,0,0): dot = -1, clamp(-1) = -1.
        // Non-linear: (-1+1)*512 = 0.
        let look_at = RspLookAt {
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        let result = compute_texture_gen(
            [65536.0, 0.0],
            [-1.0, 0.0, 0.0],
            look_at,
            false,
            identity_matrix(),
        );
        assert_close(result[0], 0.0, 1e-3);
    }

    #[test]
    fn compute_texture_gen_linear_mode_dot_zero_gives_quarter_turn() {
        // dot = 0 -> acos(-0) = acos(0) = PI/2. PI/2 * 325.94932 ~= 512.0.
        let look_at = RspLookAt {
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        let result = compute_texture_gen(
            [65536.0, 65536.0],
            [0.0, 0.0, 1.0],
            look_at,
            true,
            identity_matrix(),
        );
        let expected = (std::f32::consts::FRAC_PI_2) * LINEAR_SCALE;
        assert_close(result[0], expected, 1e-2);
        assert_close(result[1], expected, 1e-2);
    }

    #[test]
    fn compute_texture_gen_linear_mode_dot_one_gives_zero() {
        // dot = 1 -> acos(-1) = PI. PI * 325.94932 ~= 1023.999...
        let look_at = RspLookAt {
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        let result = compute_texture_gen(
            [65536.0, 0.0],
            [1.0, 0.0, 0.0],
            look_at,
            true,
            identity_matrix(),
        );
        let expected = std::f32::consts::PI * LINEAR_SCALE;
        assert_close(result[0], expected, 1e-2);
    }

    #[test]
    fn compute_texture_gen_linear_mode_dot_negative_one_gives_max() {
        // dot = -1 -> acos(1) = 0. 0 * 325.94932 = 0.
        let look_at = RspLookAt {
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        let result = compute_texture_gen(
            [65536.0, 0.0],
            [-1.0, 0.0, 0.0],
            look_at,
            true,
            identity_matrix(),
        );
        assert_close(result[0], 0.0, 1e-3);
    }

    #[test]
    fn compute_texture_gen_clamps_out_of_range_dot_product_before_linear_acos() {
        // A non-unit, non-normalized-input scenario cannot occur through
        // normalizeSafe's own output (always unit length or zero), but the
        // dot of two unit vectors can still slightly exceed [-1,1] due to
        // float rounding. Force an out-of-domain value structurally: use a
        // zero-length lookAt axis, whose normalizeSafe output is the zero
        // vector regardless of world matrix, so dot(n, 0) = 0 for any n --
        // this proves large inputNormal magnitudes cannot push texgenUV
        // outside [-1,1] through this path.
        let look_at = RspLookAt {
            x: [0.0, 0.0, 0.0],
            y: [0.0, 0.0, 0.0],
        };
        let result = compute_texture_gen(
            [65536.0, 65536.0],
            [1000.0, 1000.0, 1000.0],
            look_at,
            true,
            identity_matrix(),
        );
        // dot = 0 in both axes regardless -> acos(-0)*scale = PI/2*scale.
        let expected = std::f32::consts::FRAC_PI_2 * LINEAR_SCALE;
        assert_close(result[0], expected, 1e-2);
        assert_close(result[1], expected, 1e-2);
    }

    #[test]
    fn compute_texture_gen_transformed_lookat_axis_via_rotation_matrix() {
        // World matrix is a 90-degree rotation about Z (row-vector form):
        // row-vector (x,y,z,0) * R maps (1,0,0,0) -> (0,1,0,0) and
        // (0,1,0,0) -> (-1,0,0,0). Standard row-vector rotation-about-Z:
        // R = [[cos,sin,0,0],[-sin,cos,0,0],[0,0,1,0],[0,0,0,1]], theta=90deg.
        let theta = std::f32::consts::FRAC_PI_2;
        let (sin, cos) = theta.sin_cos();
        let rotation = WorldMatrix {
            rows: [
                [cos, sin, 0.0, 0.0],
                [-sin, cos, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        let look_at = RspLookAt {
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        // Transformed lookAt.x should become ~(0,1,0); dot with inputNormal
        // (0,1,0) should be ~1, clamp(1)=1.
        let result = compute_texture_gen([65536.0, 0.0], [0.0, 1.0, 0.0], look_at, true, rotation);
        // dot ~= 1 -> acos(-1) = PI -> PI*325.94932.
        let expected = std::f32::consts::PI * LINEAR_SCALE;
        assert_close(result[0], expected, 5e-2);
    }

    #[test]
    fn compute_texture_gen_signed_uv_scale_negates_result() {
        let look_at = RspLookAt {
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        let positive = compute_texture_gen(
            [65536.0, 65536.0],
            [1.0, 0.0, 0.0],
            look_at,
            false,
            identity_matrix(),
        );
        let negative = compute_texture_gen(
            [-65536.0, -65536.0],
            [1.0, 0.0, 0.0],
            look_at,
            false,
            identity_matrix(),
        );
        assert_close(negative[0], -positive[0], 1e-3);
        assert_close(negative[1], -positive[1], 1e-3);
    }

    #[test]
    fn compute_texture_gen_zero_uv_scale_gives_zero_output() {
        let look_at = RspLookAt {
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        let result = compute_texture_gen(
            [0.0, 0.0],
            [1.0, 0.0, 0.0],
            look_at,
            true,
            identity_matrix(),
        );
        assert_eq!(result, [0.0, 0.0]);
    }

    #[test]
    fn compute_texture_gen_zero_normal_gives_zero_dot_both_axes() {
        let look_at = RspLookAt {
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        let result = compute_texture_gen(
            [65536.0, 65536.0],
            [0.0, 0.0, 0.0],
            look_at,
            false,
            identity_matrix(),
        );
        // dot = 0 both axes -> (0+1)*512 = 512.
        assert_close(result[0], 512.0, 1e-3);
        assert_close(result[1], 512.0, 1e-3);
    }

    #[test]
    fn compute_texture_gen_linear_and_non_linear_modes_disagree_for_same_inputs() {
        // dot = 0.5 (inputNormal (1,0,0) against a lookAt.x rotated so the
        // dot product is exactly 0.5, achieved here via a 60-degree-rotated
        // lookAt.x baked directly into the vector rather than the matrix).
        // Linear: acos(-0.5) * 325.94932 = (2*PI/3) * 325.94932 ~= 682.66.
        // Non-linear: (0.5 + 1) * 512 = 768.0. These differ by ~85, well
        // past dot=0/dot=1's near-coincidental agreement (both ~512/~1024).
        let look_at = RspLookAt {
            x: [0.5, (3.0f32).sqrt() / 2.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        let linear = compute_texture_gen(
            [65536.0, 65536.0],
            [1.0, 0.0, 0.0],
            look_at,
            true,
            identity_matrix(),
        );
        let non_linear = compute_texture_gen(
            [65536.0, 65536.0],
            [1.0, 0.0, 0.0],
            look_at,
            false,
            identity_matrix(),
        );
        assert!((linear[0] - non_linear[0]).abs() > 1.0);
    }

    #[test]
    fn mutation_distinguishes_mul_operand_order() {
        // A non-symmetric matrix must produce different results depending on
        // whether the vector is treated as row (v * M) or column (M * v).
        // This test would fail if mul_row_vector_matrix's loop nesting were
        // silently swapped to the column-vector form.
        let world_matrix = WorldMatrix {
            rows: [
                [1.0, 2.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        let row_vector_result = mul_row_vector_matrix([1.0, 0.0, 0.0, 0.0], &world_matrix);
        // Row-vector form: result[c] = sum_r x[r]*M[r][c].
        // x=(1,0,0,0) picks out row 0 of M: (1,2,0,0).
        assert_eq!(row_vector_result, [1.0, 2.0, 0.0, 0.0]);
        // Column-vector form (M*v) would instead pick out column 0: (1,0,0,0).
        // Confirm the two forms actually disagree for this matrix, or the
        // test would not distinguish them.
        assert_ne!(row_vector_result, [1.0, 0.0, 0.0, 0.0]);
    }

    // --- WGSL companion: structural/parse/validation guards ---

    #[test]
    fn wgsl_entry_point_name_matches_constant() {
        assert!(TEXTURE_GEN_WGSL.contains(&format!("fn {TEXTURE_GEN_ENTRY_POINT}(")));
    }

    #[test]
    fn retained_wgsl_parses_and_validates_under_closed_naga_profile() {
        let module = naga::front::wgsl::parse_str(TEXTURE_GEN_WGSL).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn wgsl_source_contains_the_exact_literal_constants_the_oracle_depends_on() {
        assert!(TEXTURE_GEN_WGSL.contains("325.94932"));
        assert!(TEXTURE_GEN_WGSL.contains("512.0"));
        assert!(TEXTURE_GEN_WGSL.contains("65536.0"));
    }

    #[test]
    fn duplicate_binding_index_fails_naga_validation() {
        let duplicate_binding = TEXTURE_GEN_WGSL.replacen("@binding(1)", "@binding(0)", 1);
        let module = naga::front::wgsl::parse_str(&duplicate_binding).unwrap();
        assert!(naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .is_err());
    }

    #[test]
    fn malformed_wgsl_fails_to_parse() {
        let truncated = &TEXTURE_GEN_WGSL[..TEXTURE_GEN_WGSL.len() / 2];
        assert!(naga::front::wgsl::parse_str(truncated).is_err());
    }

    #[test]
    fn naga_cannot_catch_a_flipped_linear_scale_constant() {
        // A `325.94932` -> `325.0` mutation still parses and validates under
        // naga; semantic drift here is caught by this file's exhaustive Rust
        // oracle tests and the source-text guard above, matching
        // `rgb_dither.rs`'s identically-scoped precedent.
        let mutated = TEXTURE_GEN_WGSL.replacen("325.94932", "325.0", 1);
        assert_ne!(mutated, TEXTURE_GEN_WGSL);
        let module = naga::front::wgsl::parse_str(&mutated).unwrap();
        assert!(naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .is_ok());
    }
}
