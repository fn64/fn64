//! Literal port of RT64's portable RSP leaf math.
//!
//! Source: pinned MIT RT64 `5473732a822a4423b5696e7cb18fecc425a59875`,
//! `src/shared/rt64_rsp_viewport.h`, `rt64_rsp_fog.h`, `rt64_rsp_light.h`,
//! `rt64_rsp_lookat.h`, `rt64_rsp_vertex_test_z.h` (SHA-256 of the whole
//! files, matching `docs/rt64-port-inventory.json`'s `sources.port.sha256`
//! for each -- which for all five of these files is identical to
//! `sources.oracle.sha256`, so each digest below is simultaneously that
//! file's oracle and port digest, confirmed independently here by
//! `shasum -a 256` against the pinned port-commit checkout):
//! `rt64_rsp_viewport.h`
//! `0914b36810dc8ffe9d6dd8f1ffcd05b4bb229272cf61d8e8413ec313dabc06fb`;
//! `rt64_rsp_fog.h`
//! `011898d7d1b91e53fa845161931ab2e5312fb1e8fc1c369766957baf4c2fe445`;
//! `rt64_rsp_light.h`
//! `9aca26bd4195c609ccc9ee36da31e9c6ec70969ee58698cd75f1f1493872109a`;
//! `rt64_rsp_lookat.h`
//! `c6415c5cdfac3da7be8716a8996dec42d52af7e799b9fce72b6ce2f1af49f762`;
//! `rt64_rsp_vertex_test_z.h`
//! `e5204de75da748da32ef0c9b3c23f7b71dd0b8dbdb5dcf79dada88e0b425ec00`.
//! Reading MIT RT64 is an allowed clean-room source under `AGENTS.md`; see
//! `docs/DESIGN.md` "License boundary" for the wider provenance note.
//!
//! Every struct here mirrors the upstream `interop::` CPU/GPU-shared layout
//! (`HLSL_CPU`-gated in the source) with backend-neutral Rust types, and only
//! the CPU-only (`#ifndef HLSL_CPU`) formulas are ported as free functions.
//! `RT64::FixedRect`/`min`/`max`/clip-ratio integer conversion in
//! `RSPViewport::rect` are RT64-state/backend concerns out of this crate's
//! scope and are not ported.
//!
//! `Mat4::transform_point` and `Mat4::mul_vec_mat` implement the two
//! `mul()` call shapes the source uses (`mul(matrix, vector)` and
//! `mul(vector, matrix)`), per the public HLSL `mul` intrinsic contract:
//! `mul(M, v) = M·v` (`v` as a column vector) and `mul(v, M) = vᵀ·M` (`v` as
//! a row vector). This is the standard HLSL language semantic, not an
//! hlsl++-internal storage detail; hlsl++ itself is an unpopulated
//! `src/contrib/hlslpp` submodule in the pinned checkout, so no additional
//! backend-specific matrix layout is available or assumed.

/// A backend-neutral 3-component float vector, matching HLSL `float3`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub const fn splat(v: f32) -> Self {
        Self::new(v, v, v)
    }

    pub const fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }

    pub const fn scale(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }

    pub const fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z
    }
}

/// A backend-neutral 4-component float vector, matching HLSL `float4`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Vec4 {
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    pub const fn from_vec3(v: Vec3, w: f32) -> Self {
        Self::new(v.x, v.y, v.z, w)
    }

    pub const fn xyz(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }
}

/// A backend-neutral row-major 4x4 float matrix, matching HLSL `float4x4`.
///
/// `rows[i]` is row `i`; `rows[i].x/y/z/w` are that row's four columns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    pub rows: [Vec4; 4],
}

impl Mat4 {
    pub const fn from_rows(rows: [Vec4; 4]) -> Self {
        Self { rows }
    }

    /// `mul(matrix, vector)`: `vector` as a column vector, `M·v`.
    pub const fn transform_point(self, v: Vec4) -> Vec4 {
        Vec4::new(
            self.rows[0].x * v.x
                + self.rows[0].y * v.y
                + self.rows[0].z * v.z
                + self.rows[0].w * v.w,
            self.rows[1].x * v.x
                + self.rows[1].y * v.y
                + self.rows[1].z * v.z
                + self.rows[1].w * v.w,
            self.rows[2].x * v.x
                + self.rows[2].y * v.y
                + self.rows[2].z * v.z
                + self.rows[2].w * v.w,
            self.rows[3].x * v.x
                + self.rows[3].y * v.y
                + self.rows[3].z * v.z
                + self.rows[3].w * v.w,
        )
    }

    /// `mul(vector, matrix)`: `vector` as a row vector, `vᵀ·M`.
    pub const fn mul_vec_mat(self, v: Vec4) -> Vec4 {
        Vec4::new(
            v.x * self.rows[0].x
                + v.y * self.rows[1].x
                + v.z * self.rows[2].x
                + v.w * self.rows[3].x,
            v.x * self.rows[0].y
                + v.y * self.rows[1].y
                + v.z * self.rows[2].y
                + v.w * self.rows[3].y,
            v.x * self.rows[0].z
                + v.y * self.rows[1].z
                + v.z * self.rows[2].z
                + v.w * self.rows[3].z,
            v.x * self.rows[0].w
                + v.y * self.rows[1].w
                + v.z * self.rows[2].w
                + v.w * self.rows[3].w,
        )
    }
}

/// Source: `rt64_rsp_viewport.h` `struct RSPViewport`.
///
/// `rect()`/`minDepth()`/`maxDepth()` are `RT64::`-namespaced CPU helpers
/// (`FixedRect`, `RT64::FixedRect`) that belong to RT64 state/backend
/// ownership, not this crate; only the portable struct and its `identity()`
/// constructor are ported.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RspViewport {
    pub scale: Vec3,
    pub translate: Vec3,
}

impl RspViewport {
    /// Source: `RSPViewport::identity()`.
    pub const fn identity() -> Self {
        Self {
            scale: Vec3::new(1.0, 1.0, 1.0),
            translate: Vec3::new(0.0, 0.0, 0.0),
        }
    }
}

/// Source: `rt64_rsp_fog.h` `struct RSPFog`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RspFog {
    pub mul: f32,
    pub offset: f32,
}

/// Source: `rt64_rsp_light.h` `struct RSPLight`.
///
/// `kc`/`kl`/`kq` are ported as `u32` (HLSL `uint`, matching `interop::uint`)
/// even though every use site below immediately converts them to `f32`, to
/// keep the struct layout an exact field-for-field mirror of the source.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RspLight {
    pub pos_dir: Vec3,
    pub col: Vec3,
    pub colc: Vec3,
    pub kc: u32,
    pub kl: u32,
    pub kq: u32,
}

/// Source: `rt64_rsp_light.h` `computeAttenuation`.
pub fn compute_attenuation(distance: f32, light: RspLight) -> f32 {
    const A: f32 = 1.0 / 2.0;
    const B: f32 = 1.0 / 16.0;
    const C: f32 = 1.0 / 32768.0;
    const D: f32 = 1.0 / 524288.0;
    const EPSILON: f32 = 1e-6;
    let attenuation = 1.0
        / (A + B * light.kc as f32
            + C * light.kl as f32 * distance
            + D * light.kq as f32 * distance.powf(2.0));
    attenuation % (1.0 + EPSILON)
}

/// Source: `rt64_rsp_light.h` `computeNDotL`.
///
/// "This NdotL formula matches microcode behavior." (source comment.)
pub fn compute_n_dot_l(norm: Vec3, light_dir: Vec3) -> f32 {
    (norm.dot(light_dir) * 4.0).clamp(0.0, 1.0)
}

/// Source: `rt64_rsp_light.h` `computeLength`.
///
/// "This length formula matches microcode behavior." (source comment.)
pub fn compute_length(d: Vec3) -> f32 {
    (d.x * d.x + d.y * d.y + 2.0 * d.z * d.z).sqrt()
}

/// Source: `rt64_rsp_light.h` `computePosLight`.
pub fn compute_pos_light(pos: Vec3, norm: Vec3, light: RspLight, world_matrix: Mat4) -> Vec3 {
    let world_vertex_pos = world_matrix
        .transform_point(Vec4::from_vec3(pos, 1.0))
        .xyz();
    let mut world_light_dir = light.pos_dir.sub(world_vertex_pos);
    let world_light_dist = compute_length(world_light_dir);
    if world_light_dist > 0.0 {
        world_light_dir = world_light_dir.scale(1.0 / world_light_dist);
    }

    let local_light_dir = world_matrix
        .mul_vec_mat(Vec4::from_vec3(world_light_dir, 0.0))
        .xyz();
    let weight =
        compute_n_dot_l(norm, local_light_dir) * compute_attenuation(world_light_dist, light);
    light.col.scale(weight)
}

/// Source: `rt64_rsp_light.h` `computeDirLight`.
pub fn compute_dir_light(norm: Vec3, light: RspLight, world_matrix: Mat4) -> Vec3 {
    let mut local_light_dir = world_matrix
        .mul_vec_mat(Vec4::from_vec3(light.pos_dir, 0.0))
        .xyz();
    let local_light_length = (local_light_dir.x * local_light_dir.x
        + local_light_dir.y * local_light_dir.y
        + local_light_dir.z * local_light_dir.z)
        .sqrt();
    if local_light_length > 0.0 {
        local_light_dir = local_light_dir.scale(1.0 / local_light_length);
    }

    light.col.scale(norm.dot(local_light_dir).max(0.0))
}

/// Source: `rt64_rsp_lookat.h` `struct RSPLookAt`, plus its
/// `RSP_LOOKAT_INDEX_*` bit-field constants.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RspLookAt {
    pub x: Vec3,
    pub y: Vec3,
}

pub const RSP_LOOKAT_INDEX_ENABLED: u32 = 0x1;
pub const RSP_LOOKAT_INDEX_LINEAR: u32 = 0x2;
pub const RSP_LOOKAT_INDEX_SHIFT: u32 = 2;

/// Source: `rt64_rsp_vertex_test_z.h` `struct RSPVertexTestZCB`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RspVertexTestZCb {
    pub resolution_scale: [f32; 2],
    pub vertex_index: u32,
    pub src_index_start: u32,
    pub dst_index_start: u32,
    pub index_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-5;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < EPS, "{a} !~= {b}");
    }

    fn approx_vec3(a: Vec3, b: Vec3) {
        approx(a.x, b.x);
        approx(a.y, b.y);
        approx(a.z, b.z);
    }

    /// A rotate-90-about-Z-then-translate-by-(10,0,0) matrix, row-major:
    /// `x' = -y, y' = x, z' = z`, then `x' += 10`. Chosen because it is not
    /// diagonal, so it exercises both `transform_point`'s and `mul_vec_mat`'s
    /// full 4x4 dot products rather than degenerating to a single term.
    fn rotate_z90_translate_x10() -> Mat4 {
        Mat4::from_rows([
            Vec4::new(0.0, -1.0, 0.0, 10.0),
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ])
    }

    // --- Vec3/Vec4/Mat4 primitives -----------------------------------

    #[test]
    fn vec3_arithmetic_matches_componentwise_definition() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(4.0, -1.0, 0.5);
        approx_vec3(a.sub(b), Vec3::new(-3.0, 3.0, 2.5));
        approx_vec3(a.scale(2.0), Vec3::new(2.0, 4.0, 6.0));
        approx(a.dot(b), 1.0 * 4.0 - 2.0 * 1.0 + 3.0 * 0.5);
        approx_vec3(Vec3::splat(7.0), Vec3::new(7.0, 7.0, 7.0));
    }

    #[test]
    fn mat4_identity_is_a_fixed_point_for_both_mul_orders() {
        let identity = Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ]);
        let v = Vec4::new(1.0, 2.0, 3.0, 4.0);
        assert_eq!(identity.transform_point(v), v);
        assert_eq!(identity.mul_vec_mat(v), v);
    }

    #[test]
    fn mat4_transform_point_applies_rotation_and_translation() {
        let m = rotate_z90_translate_x10();
        // point (1,0,0) rotates to (0,1,0) then translates by (10,0,0).
        let out = m.transform_point(Vec4::new(1.0, 0.0, 0.0, 1.0));
        approx(out.x, 10.0);
        approx(out.y, 1.0);
        approx(out.z, 0.0);
        approx(out.w, 1.0);
    }

    #[test]
    fn mat4_mul_vec_mat_is_the_transpose_orientation_of_transform_point() {
        // mul(v, M) = v^T * M; for a pure-rotation direction (w=0) this is
        // the transpose of the rotation used by transform_point, so a
        // world +Y direction maps to local +X here (opposite of how a
        // world +X point's rotation component maps to local +Y above).
        let m = rotate_z90_translate_x10();
        let out = m.mul_vec_mat(Vec4::new(0.0, 1.0, 0.0, 0.0));
        approx(out.x, 1.0);
        approx(out.y, 0.0);
        approx(out.z, 0.0);
        approx(out.w, 0.0);
    }

    // --- RspViewport ----------------------------------------------------

    #[test]
    fn viewport_identity_has_unit_scale_and_zero_translate() {
        let v = RspViewport::identity();
        approx_vec3(v.scale, Vec3::new(1.0, 1.0, 1.0));
        approx_vec3(v.translate, Vec3::new(0.0, 0.0, 0.0));
    }

    // --- compute_attenuation --------------------------------------------

    fn light_with_coeffs(kc: u32, kl: u32, kq: u32) -> RspLight {
        RspLight {
            pos_dir: Vec3::default(),
            col: Vec3::default(),
            colc: Vec3::default(),
            kc,
            kl,
            kq,
        }
    }

    #[test]
    fn attenuation_zero_coefficients_wraps_through_fmod() {
        // 1/(1/2) = 2.0; fmod(2.0, 1.000001) = 2.0 - 1.000001.
        let got = compute_attenuation(10.0, light_with_coeffs(0, 0, 0));
        approx(got, 0.999999);
    }

    #[test]
    fn attenuation_zero_distance_uses_constant_term_only() {
        // 1/(0.5 + (1/16)*2) = 1/0.625 = 1.6; fmod(1.6, 1.000001) = 1.6 - 1.000001.
        let got = compute_attenuation(0.0, light_with_coeffs(2, 0, 0));
        approx(got, 0.599999);
    }

    #[test]
    fn attenuation_linear_term_stays_below_fmod_wrap() {
        // C*kl*distance = (1/32768)*32768*1 = 1.0; 1/(0.5+1.0) = 0.6666667,
        // already below 1.000001 so fmod is the identity.
        let got = compute_attenuation(1.0, light_with_coeffs(0, 32768, 0));
        approx(got, 0.6666667);
    }

    #[test]
    fn attenuation_quadratic_term_scales_by_distance_squared() {
        let got = compute_attenuation(100.0, light_with_coeffs(1, 1, 1));
        approx(got, 0.7104965);
    }

    // --- compute_n_dot_l --------------------------------------------------

    #[test]
    fn n_dot_l_clamps_below_zero() {
        approx(
            compute_n_dot_l(Vec3::new(1.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)),
            0.0,
        );
        approx(
            compute_n_dot_l(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)),
            0.0,
        );
    }

    #[test]
    fn n_dot_l_scales_dot_by_four_before_clamping_to_one() {
        // dot = 0.2 along a shared axis; 0.2*4 = 0.8, inside [0,1].
        let n = Vec3::new(0.2, 0.0, 0.0);
        let l = Vec3::new(1.0, 0.0, 0.0);
        approx(compute_n_dot_l(n, l), 0.8);
    }

    #[test]
    fn n_dot_l_clamps_at_the_exact_quarter_boundary_and_above() {
        // dot = 0.25 -> 0.25*4 = 1.0 exactly: the clamp boundary.
        let n = Vec3::new(0.25, 0.0, 0.0);
        let l = Vec3::new(1.0, 0.0, 0.0);
        approx(compute_n_dot_l(n, l), 1.0);
        // dot = 0.5 -> 2.0, clamps down to 1.0.
        let n2 = Vec3::new(0.5, 0.0, 0.0);
        approx(compute_n_dot_l(n2, l), 1.0);
    }

    // --- compute_length -----------------------------------------------

    #[test]
    fn length_matches_pythagorean_form_when_z_is_zero() {
        approx(compute_length(Vec3::new(3.0, 4.0, 0.0)), 5.0);
    }

    #[test]
    fn length_double_weights_the_z_component() {
        // sqrt(2)*z, not z, because the z term is doubled before the sqrt.
        approx(
            compute_length(Vec3::new(0.0, 0.0, 5.0)),
            5.0 * std::f32::consts::SQRT_2,
        );
    }

    #[test]
    fn length_mixed_components_match_independently_derived_value() {
        // sqrt(1^2 + 2^2 + 2*3^2) = sqrt(1+4+18) = sqrt(23).
        approx(compute_length(Vec3::new(1.0, 2.0, 3.0)), 23f32.sqrt());
    }

    // --- compute_pos_light / compute_dir_light ---------------------------

    #[test]
    fn pos_light_zero_distance_skips_normalization_and_yields_zero_ndotl() {
        // Vertex sits exactly at the light position: worldLightDist == 0, so
        // the source's `if (worldLightDist > 0)` branch is skipped and the
        // un-normalized zero vector is used, giving NdotL == 0 regardless of
        // norm, and the result color is exactly zero.
        let light = RspLight {
            pos_dir: Vec3::new(5.0, 0.0, 0.0),
            col: Vec3::new(1.0, 1.0, 1.0),
            colc: Vec3::default(),
            kc: 0,
            kl: 0,
            kq: 0,
        };
        let identity = Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 5.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ]);
        let result = compute_pos_light(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            light,
            identity,
        );
        approx_vec3(result, Vec3::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn pos_light_full_pipeline_matches_independently_derived_value() {
        // pos=(1,0,0) in local space -> world (10,1,0) under rotate+translate.
        // light at world (10,5,0) -> world_light_dir=(0,4,0), dist=4,
        // normalized (0,1,0) -> back to local space via mul(v,M) = (1,0,0).
        // norm=(1,0,0) is exactly aligned, so NdotL clamps to 1.0.
        // attenuation(4.0; kc=4,kl=8192,kq=0) = 1/(0.5+0.25+0.25) = 1/0.75.
        let m = rotate_z90_translate_x10();
        let light = RspLight {
            pos_dir: Vec3::new(10.0, 5.0, 0.0),
            col: Vec3::new(1.0, 0.5, 0.25),
            colc: Vec3::default(),
            kc: 4,
            kl: 8192,
            kq: 0,
        };
        let result =
            compute_pos_light(Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), light, m);
        approx_vec3(result, Vec3::new(4.0 / 7.0, 2.0 / 7.0, 1.0 / 7.0));
    }

    #[test]
    fn dir_light_zero_length_skips_normalization() {
        // A zero direction has localLightLength == 0, so the source's
        // `if (localLightLength > 0)` branch is skipped; dot with any norm
        // is 0, and max(0, 0) leaves the result at zero.
        let light = RspLight {
            pos_dir: Vec3::new(0.0, 0.0, 0.0),
            col: Vec3::new(1.0, 1.0, 1.0),
            colc: Vec3::default(),
            kc: 0,
            kl: 0,
            kq: 0,
        };
        let identity = Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ]);
        let result = compute_dir_light(Vec3::new(1.0, 0.0, 0.0), light, identity);
        approx_vec3(result, Vec3::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn dir_light_full_pipeline_matches_independently_derived_value() {
        // world +Y direction rotates (via mul(v,M)) to local +X; norm=(1,0,0)
        // is aligned, so dot = 1.0 and the result equals the light color.
        let m = rotate_z90_translate_x10();
        let light = RspLight {
            pos_dir: Vec3::new(0.0, 1.0, 0.0),
            col: Vec3::new(1.0, 0.5, 0.25),
            colc: Vec3::default(),
            kc: 0,
            kl: 0,
            kq: 0,
        };
        let result = compute_dir_light(Vec3::new(1.0, 0.0, 0.0), light, m);
        approx_vec3(result, Vec3::new(1.0, 0.5, 0.25));
    }

    #[test]
    fn dir_light_clamps_to_zero_when_facing_away() {
        let m = rotate_z90_translate_x10();
        let light = RspLight {
            pos_dir: Vec3::new(0.0, 1.0, 0.0),
            col: Vec3::new(1.0, 0.5, 0.25),
            colc: Vec3::default(),
            kc: 0,
            kl: 0,
            kq: 0,
        };
        // norm=(-1,0,0) faces away from the local +X light direction.
        let result = compute_dir_light(Vec3::new(-1.0, 0.0, 0.0), light, m);
        approx_vec3(result, Vec3::new(0.0, 0.0, 0.0));
    }

    // --- RspLookAt / RspVertexTestZCb: layout boundary tests -------------

    #[test]
    fn lookat_index_constants_match_source_bit_layout() {
        assert_eq!(RSP_LOOKAT_INDEX_ENABLED, 0x1);
        assert_eq!(RSP_LOOKAT_INDEX_LINEAR, 0x2);
        assert_eq!(RSP_LOOKAT_INDEX_SHIFT, 2);
    }

    #[test]
    fn lookat_struct_retains_both_axes_independently() {
        let l = RspLookAt {
            x: Vec3::new(1.0, 0.0, 0.0),
            y: Vec3::new(0.0, 1.0, 0.0),
        };
        approx_vec3(l.x, Vec3::new(1.0, 0.0, 0.0));
        approx_vec3(l.y, Vec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn vertex_test_z_cb_retains_every_field_independently() {
        let cb = RspVertexTestZCb {
            resolution_scale: [0.5, 0.25],
            vertex_index: 1,
            src_index_start: 2,
            dst_index_start: 3,
            index_count: 4,
        };
        assert_eq!(cb.resolution_scale, [0.5, 0.25]);
        assert_eq!(cb.vertex_index, 1);
        assert_eq!(cb.src_index_start, 2);
        assert_eq!(cb.dst_index_start, 3);
        assert_eq!(cb.index_count, 4);
    }
}
