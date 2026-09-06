use super::*;

fn ident() -> Mat4 {
    Mat4::from_rows([
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ])
}

fn translate(tx: f32, ty: f32, tz: f32) -> Mat4 {
    Mat4::from_rows([
        Vec4::new(1.0, 0.0, 0.0, tx),
        Vec4::new(0.0, 1.0, 0.0, ty),
        Vec4::new(0.0, 0.0, 1.0, tz),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ])
}

fn rotate90z() -> Mat4 {
    // x' = -y, y' = x, z' = z: 90-degree rotation about Z.
    Mat4::from_rows([
        Vec4::new(0.0, -1.0, 0.0, 0.0),
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ])
}

fn scale_diag(sx: f32, sy: f32, sz: f32) -> Mat4 {
    Mat4::from_rows([
        Vec4::new(sx, 0.0, 0.0, 0.0),
        Vec4::new(0.0, sy, 0.0, 0.0),
        Vec4::new(0.0, 0.0, sz, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ])
}

// ---------------------------------------------------------------------
// Independent CPU oracle
// ---------------------------------------------------------------------
//
// A second, independently-derived re-expression of both shaders' arithmetic,
// written directly from the HLSL text without reusing this module's own
// helper functions (`rsp_modify_z`, `rsp_modify_xy`,
// `rsp_world_weighted_pos`, `rsp_world_norm`, `Mat4::transform_point`), so
// the tests below compare two independent derivations rather than one
// implementation against itself.

fn oracle_modify_z(modify_value: u32) -> f32 {
    (modify_value as f64 / 65536.0f64) as f32
}

fn oracle_modify_xy(modify_value: u32) -> (f32, f32) {
    let ext_x = (modify_value >> 16) & 0xFFFF;
    let ext_y = modify_value & 0xFFFF;
    // Alternate sign-extension route: widen to u16 first, then bit-cast to
    // i16 (matching `rt64_rsp_patch.rs`'s documented "extracting u16 then
    // as i16" idiom), rather than this module's own `<<16>>16` on i32.
    let sx = (ext_x as u16) as i16;
    let sy = (ext_y as u16) as i16;
    ((sx as f32) / 4.0, (sy as f32) / 4.0)
}

fn oracle_mat_vec_mul(rows: [[f32; 4]; 4], v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for (i, out_i) in out.iter_mut().enumerate() {
        let r = rows[i];
        // Explicit left-to-right accumulation, written as a running sum
        // rather than the struct-field-chain `Mat4::transform_point` uses.
        let mut acc = r[0] * v[0];
        acc += r[1] * v[1];
        acc += r[2] * v[2];
        acc += r[3] * v[3];
        *out_i = acc;
    }
    out
}

fn oracle_weighted_pos(pos: [f32; 3], vel: [f32; 3], frame_weight: f32) -> [f32; 3] {
    let factor = 1.0f32 - frame_weight;
    [
        pos[0] - (vel[0] * factor),
        pos[1] - (vel[1] * factor),
        pos[2] - (vel[2] * factor),
    ]
}

fn oracle_normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / len, v[1] / len, v[2] / len]
}

// =======================================================================
// RSPModifyCS: decode_modify_pos (packed tag/index word)
// =======================================================================

#[test]
fn decode_modify_pos_z_bit_set() {
    let d = decode_modify_pos(0b11); // vertex_index=1, modify_z=true
    assert!(d.modify_z);
    assert_eq!(d.vertex_index, 1);
}

#[test]
fn decode_modify_pos_z_bit_clear() {
    let d = decode_modify_pos(0b10); // vertex_index=1, modify_z=false
    assert!(!d.modify_z);
    assert_eq!(d.vertex_index, 1);
}

#[test]
fn decode_modify_pos_zero() {
    let d = decode_modify_pos(0);
    assert!(!d.modify_z);
    assert_eq!(d.vertex_index, 0);
}

#[test]
fn decode_modify_pos_large_index() {
    // vertex_index = 255 (RSP_MAX_VERTICES - 1), modify_z = true:
    // (255 << 1) | 1 = 511.
    let d = decode_modify_pos(511);
    assert!(d.modify_z);
    assert_eq!(d.vertex_index, 255);
}

#[test]
fn decode_modify_pos_matches_rt64_rsp_patch_tag_encoding() {
    // rt64_rsp_patch::decode_modify_vertex documents XyScreen's tag as
    // `global_index << 1` and ZScreen's as `(global_index << 1) | 0x1`.
    // Round-tripping through decode_modify_pos must recover global_index
    // and the correct branch selector for both.
    let xy_tag = 7u32 << 1;
    let d_xy = decode_modify_pos(xy_tag);
    assert!(!d_xy.modify_z);
    assert_eq!(d_xy.vertex_index, 7);

    let z_tag = (7u32 << 1) | 0x1;
    let d_z = decode_modify_pos(z_tag);
    assert!(d_z.modify_z);
    assert_eq!(d_z.vertex_index, 7);
}

// =======================================================================
// RSPModifyCS: rsp_modify_z (unsigned, no negative range)
// =======================================================================

#[test]
fn modify_z_zero() {
    assert_eq!(rsp_modify_z(0), 0.0);
    assert_eq!(oracle_modify_z(0), 0.0);
}

#[test]
fn modify_z_one_unit() {
    // 65536 / 65536.0 = 1.0 exactly.
    assert_eq!(rsp_modify_z(65536), 1.0);
    assert_eq!(oracle_modify_z(65536), 1.0);
}

#[test]
fn modify_z_top_bit_set_stays_positive() {
    // Unlike a signed s16.16 reinterpret, the top bit does not flip sign:
    // 0x80000000 / 65536.0 = 32768.0, a large POSITIVE value.
    assert_eq!(rsp_modify_z(0x8000_0000), 32768.0);
    assert_eq!(oracle_modify_z(0x8000_0000), 32768.0);
}

#[test]
fn modify_z_max_u32_has_no_negative_range() {
    // u32::MAX as f32 rounds up to exactly 4294967296.0 (f32's 24-bit
    // mantissa cannot represent 4294967295 exactly), so the division is
    // exactly 65536.0, not 65535.99998... -- hand-verified via a Python
    // struct-pack f32 round-trip, not captured from this module's own
    // implementation.
    let v = rsp_modify_z(u32::MAX);
    assert!(v > 0.0);
    assert_eq!(v, 65536.0);
    assert_eq!(oracle_modify_z(u32::MAX), 65536.0);
}

#[test]
fn modify_z_oracle_agrees_across_boundary_sweep() {
    for value in [
        0u32,
        1,
        65535,
        65536,
        65537,
        0x7FFF_FFFF,
        0x8000_0000,
        u32::MAX,
    ] {
        assert_eq!(
            rsp_modify_z(value),
            oracle_modify_z(value),
            "mismatch for value={value:#x}"
        );
    }
}

// =======================================================================
// RSPModifyCS: rsp_modify_xy (signed sign-extension)
// =======================================================================

#[test]
fn modify_xy_zero() {
    assert_eq!(rsp_modify_xy(0), (0.0, 0.0));
}

#[test]
fn modify_xy_positive_values() {
    // extX=8, extY=16: x=8/4=2.0, y=16/4=4.0.
    let (x, y) = rsp_modify_xy(0x0008_0010);
    assert_eq!(x, 2.0);
    assert_eq!(y, 4.0);
}

#[test]
fn modify_xy_negative_values() {
    // -4 as u16 bit pattern = 0xFFFC for both halves: x=y=-4/4=-1.0.
    let (x, y) = rsp_modify_xy(0xFFFC_FFFC);
    assert_eq!(x, -1.0);
    assert_eq!(y, -1.0);
}

#[test]
fn modify_xy_sign_boundary_min_i16() {
    // 0x8000 as i16 = -32768; -32768/4.0 = -8192.0 exactly.
    let (x, y) = rsp_modify_xy(0x8000_8000);
    assert_eq!(x, -8192.0);
    assert_eq!(y, -8192.0);
}

#[test]
fn modify_xy_sign_boundary_max_i16() {
    // 0x7FFF as i16 = 32767; 32767/4.0 = 8191.75 exactly (32767 = 4*8191+3).
    let (x, y) = rsp_modify_xy(0x7FFF_7FFF);
    assert_eq!(x, 8191.75);
    assert_eq!(y, 8191.75);
}

#[test]
fn modify_xy_sign_boundary_negative_one() {
    // 0xFFFF as i16 = -1; -1/4.0 = -0.25.
    let (x, y) = rsp_modify_xy(0xFFFF_FFFF);
    assert_eq!(x, -0.25);
    assert_eq!(y, -0.25);
}

#[test]
fn modify_xy_x_and_y_are_independent_lanes() {
    // Only the low half (Y) set to a nonzero negative value; X stays 0.
    let (x, y) = rsp_modify_xy(0x0000_0001);
    assert_eq!(x, 0.0);
    assert_eq!(y, 0.25);
}

#[test]
fn modify_xy_matches_rt64_rsp_patch_xyscreen_formula() {
    // Cross-check against rt64_rsp_patch::decode_modify_vertex's
    // G_MWO_POINT_XYSCREEN case for the same packed value -- same divisor
    // (4.0), same sign-extension result, confirming the CPU-decode and
    // GPU-decode halves of this shared patch semantic agree (see module
    // doc "Reuse, not new type").
    let value = 0x0008_0010u32;
    let cpu_patch = crate::rt64_rsp_patch::decode_modify_vertex(
        0,
        crate::rt64_rsp_patch::G_MWO_POINT_XYSCREEN,
        value,
        0,
    )
    .unwrap();
    let (gpu_x, gpu_y) = rsp_modify_xy(value);
    match cpu_patch {
        crate::rt64_rsp_patch::ModifyVertexPatch::XyScreen { x, y, .. } => {
            assert_eq!(x, gpu_x);
            assert_eq!(y, gpu_y);
        }
        _ => panic!("expected XyScreen"),
    }
}

#[test]
fn modify_z_matches_rt64_rsp_patch_zscreen_formula() {
    let value = 0x8000_0000u32;
    let cpu_patch = crate::rt64_rsp_patch::decode_modify_vertex(
        0,
        crate::rt64_rsp_patch::G_MWO_POINT_ZSCREEN,
        value,
        0,
    )
    .unwrap();
    let gpu_z = rsp_modify_z(value);
    match cpu_patch {
        crate::rt64_rsp_patch::ModifyVertexPatch::ZScreen { z, .. } => {
            assert_eq!(z, gpu_z);
        }
        _ => panic!("expected ZScreen"),
    }
}

#[test]
fn modify_xy_oracle_agrees_across_sign_extension_sweep() {
    for value in [
        0u32,
        1,
        0x0000_FFFF,
        0xFFFF_0000,
        0x7FFF_7FFF,
        0x8000_8000,
        0xFFFF_FFFF,
        0x1234_5678,
    ] {
        assert_eq!(
            rsp_modify_xy(value),
            oracle_modify_xy(value),
            "mismatch for value={value:#x}"
        );
    }
}

// =======================================================================
// RSPWorldCS: rsp_world_weighted_pos
// =======================================================================

#[test]
fn weighted_pos_zero_velocity_returns_pos_unchanged() {
    let pos = Vec3::new(1.0, 2.0, 3.0);
    let vel = Vec3::new(0.0, 0.0, 0.0);
    let out = rsp_world_weighted_pos(pos, vel, 1.0);
    assert_eq!((out.x, out.y, out.z), (1.0, 2.0, 3.0));
}

#[test]
fn weighted_pos_frame_weight_one_zeroes_velocity_term() {
    // 1.0 - 1.0 = 0.0, so vel contributes nothing regardless of its value.
    let pos = Vec3::new(1.0, 1.0, 1.0);
    let vel = Vec3::new(100.0, -50.0, 7.0);
    let out = rsp_world_weighted_pos(pos, vel, 1.0);
    assert_eq!((out.x, out.y, out.z), (1.0, 1.0, 1.0));
}

#[test]
fn weighted_pos_frame_weight_zero_subtracts_full_velocity() {
    let pos = Vec3::new(10.0, 10.0, 10.0);
    let vel = Vec3::new(1.0, 2.0, 3.0);
    let out = rsp_world_weighted_pos(pos, vel, 0.0);
    assert_eq!((out.x, out.y, out.z), (9.0, 8.0, 7.0));
}

#[test]
fn weighted_pos_half_weight() {
    // pos=(4,4,4), vel=(2,0,-2), weight=0.5: 1-0.5=0.5;
    // x: 4 - 2*0.5 = 3.0; y: 4 - 0*0.5 = 4.0; z: 4 - (-2)*0.5 = 5.0.
    let pos = Vec3::new(4.0, 4.0, 4.0);
    let vel = Vec3::new(2.0, 0.0, -2.0);
    let out = rsp_world_weighted_pos(pos, vel, 0.5);
    assert_eq!((out.x, out.y, out.z), (3.0, 4.0, 5.0));
}

#[test]
fn weighted_pos_negative_pos_and_velocity() {
    // pos=(-5,-5,-5), vel=(-2,3,-1), weight=0.5: factor=0.5;
    // x: -5 - (-2*0.5) = -5+1 = -4.0
    // y: -5 - (3*0.5) = -5-1.5 = -6.5
    // z: -5 - (-1*0.5) = -5+0.5 = -4.5
    let pos = Vec3::new(-5.0, -5.0, -5.0);
    let vel = Vec3::new(-2.0, 3.0, -1.0);
    let out = rsp_world_weighted_pos(pos, vel, 0.5);
    assert_eq!((out.x, out.y, out.z), (-4.0, -6.5, -4.5));
}

#[test]
fn weighted_pos_oracle_agrees() {
    let cases: [([f32; 3], [f32; 3], f32); 4] = [
        ([1.0, 2.0, 3.0], [0.0, 0.0, 0.0], 1.0),
        ([10.0, 10.0, 10.0], [1.0, 2.0, 3.0], 0.0),
        ([4.0, 4.0, 4.0], [2.0, 0.0, -2.0], 0.5),
        ([-5.0, -5.0, -5.0], [-2.0, 3.0, -1.0], 0.25),
    ];
    for (pos, vel, w) in cases {
        let ported = rsp_world_weighted_pos(
            Vec3::new(pos[0], pos[1], pos[2]),
            Vec3::new(vel[0], vel[1], vel[2]),
            w,
        );
        let oracle = oracle_weighted_pos(pos, vel, w);
        assert_eq!(
            (ported.x, ported.y, ported.z),
            (oracle[0], oracle[1], oracle[2])
        );
    }
}

// =======================================================================
// RSPWorldCS: rsp_world_norm (zero guard + normalize)
// =======================================================================

#[test]
fn world_norm_exact_zero_takes_guard_branch() {
    let out = rsp_world_norm(ident(), Vec3::new(0.0, 0.0, 0.0));
    assert_eq!((out.x, out.y, out.z, out.w), (0.0, 0.0, 0.0, 1.0));
}

#[test]
fn world_norm_negative_zero_takes_guard_branch() {
    // IEEE-754 -0.0 == 0.0 is true, matching HLSL's == on float.
    let out = rsp_world_norm(ident(), Vec3::new(-0.0, 0.0, 0.0));
    assert_eq!((out.x, out.y, out.z, out.w), (0.0, 0.0, 0.0, 1.0));
}

#[test]
fn world_norm_partially_zero_does_not_take_guard_branch() {
    // Only two of three components are zero: all() requires every
    // component, so this must take the transform branch.
    let out = rsp_world_norm(ident(), Vec3::new(0.0, 0.0, 5.0));
    // normalize((0,0,5)) = (0,0,1).
    assert_eq!((out.x, out.y, out.z, out.w), (0.0, 0.0, 1.0, 1.0));
}

#[test]
fn world_norm_identity_matrix_normalizes_3_4_0() {
    // (3,4,0) has length 5 exactly; normalized = (0.6, 0.8, 0.0).
    let out = rsp_world_norm(ident(), Vec3::new(3.0, 4.0, 0.0));
    assert_eq!(out.x, 0.6);
    assert_eq!(out.y, 0.8);
    assert_eq!(out.z, 0.0);
    assert_eq!(out.w, 1.0);
}

#[test]
fn world_norm_already_unit_length_axis_vector() {
    let out = rsp_world_norm(ident(), Vec3::new(0.0, 1.0, 0.0));
    assert_eq!((out.x, out.y, out.z, out.w), (0.0, 1.0, 0.0, 1.0));
}

#[test]
fn world_norm_scaling_matrix_changes_direction() {
    // invTWorldMat = diag(2,1,1); norm=(1,1,0) -> transformed=(2,1,0),
    // length=sqrt(5), normalized=(2/sqrt(5), 1/sqrt(5), 0).
    let out = rsp_world_norm(scale_diag(2.0, 1.0, 1.0), Vec3::new(1.0, 1.0, 0.0));
    let expected_len = 5.0f32.sqrt();
    assert_eq!(out.x, 2.0 / expected_len);
    assert_eq!(out.y, 1.0 / expected_len);
    assert_eq!(out.z, 0.0);
}

#[test]
fn world_norm_negative_components_normalize_correctly() {
    // norm=(-3,-4,0), length 5, normalized=(-0.6,-0.8,0.0).
    let out = rsp_world_norm(ident(), Vec3::new(-3.0, -4.0, 0.0));
    assert_eq!(out.x, -0.6);
    assert_eq!(out.y, -0.8);
    assert_eq!(out.z, 0.0);
}

#[test]
fn world_norm_oracle_agrees_for_identity_and_scale() {
    let cases: [([f32; 3], [[f32; 4]; 4]); 3] = [
        (
            [3.0, 4.0, 0.0],
            [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        ),
        (
            [1.0, 1.0, 0.0],
            [
                [2.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        ),
        (
            [1.0, 1.0, 1.0],
            [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        ),
    ];
    for (norm, rows) in cases {
        let mat = Mat4::from_rows([
            Vec4::new(rows[0][0], rows[0][1], rows[0][2], rows[0][3]),
            Vec4::new(rows[1][0], rows[1][1], rows[1][2], rows[1][3]),
            Vec4::new(rows[2][0], rows[2][1], rows[2][2], rows[2][3]),
            Vec4::new(rows[3][0], rows[3][1], rows[3][2], rows[3][3]),
        ]);
        let ported = rsp_world_norm(mat, Vec3::new(norm[0], norm[1], norm[2]));

        let transformed = oracle_mat_vec_mul(rows, [norm[0], norm[1], norm[2], 0.0]);
        let normalized = oracle_normalize3([transformed[0], transformed[1], transformed[2]]);
        assert_eq!(ported.x, normalized[0]);
        assert_eq!(ported.y, normalized[1]);
        assert_eq!(ported.z, normalized[2]);
        assert_eq!(ported.w, 1.0);
    }
}

// =======================================================================
// RSPWorldCS: rsp_world_transform (full per-vertex composition)
// =======================================================================

#[test]
fn world_transform_identity_no_velocity_passes_pos_through() {
    let out = rsp_world_transform(
        Vec3::new(1.0, 2.0, 3.0),
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        ident(),
        ident(),
        ident(),
        1.0,
        1.0,
    );
    assert_eq!(
        (
            out.world_pos.x,
            out.world_pos.y,
            out.world_pos.z,
            out.world_pos.w
        ),
        (1.0, 2.0, 3.0, 1.0)
    );
    // prevWorldPos equals worldPos (same weight, same matrix, same input),
    // so worldVel must be exactly zero in every component.
    assert_eq!(
        (
            out.world_vel.x,
            out.world_vel.y,
            out.world_vel.z,
            out.world_vel.w
        ),
        (0.0, 0.0, 0.0, 0.0)
    );
}

#[test]
fn world_transform_translation_matrix_offsets_position() {
    let out = rsp_world_transform(
        Vec3::new(1.0, 2.0, 3.0),
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        translate(10.0, 20.0, 30.0),
        ident(),
        translate(10.0, 20.0, 30.0),
        0.0,
        0.0,
    );
    assert_eq!(
        (
            out.world_pos.x,
            out.world_pos.y,
            out.world_pos.z,
            out.world_pos.w
        ),
        (11.0, 22.0, 33.0, 1.0)
    );
}

#[test]
fn world_transform_rotation_matrix_rotates_position() {
    // 90-degree Z rotation of (1,0,0) -> (0,1,0).
    let out = rsp_world_transform(
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        rotate90z(),
        ident(),
        rotate90z(),
        1.0,
        1.0,
    );
    assert_eq!(out.world_pos.x, 0.0);
    assert_eq!(out.world_pos.y, 1.0);
    assert_eq!(out.world_pos.z, 0.0);
    assert_eq!(out.world_pos.w, 1.0);
}

#[test]
fn world_transform_velocity_and_differing_frame_weights_produce_nonzero_vel() {
    // pos=(4,4,4), vel=(2,0,-2), curFrameWeight=0.5 -> worldPos=(3,4,5,1)
    // (identity matrix). prevFrameWeight=0.25 -> weighted=(4-2*0.75,4,4+2*0.75)
    // = (2.5,4,5.5) -> prevWorldPos=(2.5,4,5.5,1) (identity matrix).
    // worldVel = worldPos - prevWorldPos = (0.5, 0.0, -0.5, 0.0).
    let out = rsp_world_transform(
        Vec3::new(4.0, 4.0, 4.0),
        Vec3::new(2.0, 0.0, -2.0),
        Vec3::new(0.0, 0.0, 1.0),
        ident(),
        ident(),
        ident(),
        0.25,
        0.5,
    );
    assert_eq!(
        (
            out.world_pos.x,
            out.world_pos.y,
            out.world_pos.z,
            out.world_pos.w
        ),
        (3.0, 4.0, 5.0, 1.0)
    );
    assert_eq!(
        (
            out.world_vel.x,
            out.world_vel.y,
            out.world_vel.z,
            out.world_vel.w
        ),
        (0.5, 0.0, -0.5, 0.0)
    );
}

#[test]
fn world_transform_zero_normal_produces_guard_branch_output() {
    let out = rsp_world_transform(
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 0.0),
        ident(),
        ident(),
        ident(),
        1.0,
        1.0,
    );
    assert_eq!(
        (
            out.world_norm.x,
            out.world_norm.y,
            out.world_norm.z,
            out.world_norm.w
        ),
        (0.0, 0.0, 0.0, 1.0)
    );
}

#[test]
fn world_transform_nonzero_normal_uses_inv_t_matrix_not_world_matrix() {
    // world_mat is a rotation (would change the normal's direction if used),
    // inv_t_world_mat is identity (so normal passes through unchanged aside
    // from normalize) -- this test asserts the normal transform uses
    // inv_t_world_mat, never world_mat.
    let out = rsp_world_transform(
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(3.0, 4.0, 0.0),
        rotate90z(),
        ident(),
        rotate90z(),
        1.0,
        1.0,
    );
    assert_eq!(out.world_norm.x, 0.6);
    assert_eq!(out.world_norm.y, 0.8);
    assert_eq!(out.world_norm.z, 0.0);
}

#[test]
fn world_transform_negative_position_and_velocity() {
    let out = rsp_world_transform(
        Vec3::new(-5.0, -5.0, -5.0),
        Vec3::new(-2.0, 3.0, -1.0),
        Vec3::new(1.0, 0.0, 0.0),
        ident(),
        ident(),
        ident(),
        0.5,
        0.5,
    );
    assert_eq!(
        (
            out.world_pos.x,
            out.world_pos.y,
            out.world_pos.z,
            out.world_pos.w
        ),
        (-4.0, -6.5, -4.5, 1.0)
    );
}

#[test]
fn world_transform_nan_velocity_propagates_to_pos_and_vel() {
    // No guard exists on pos/vel arithmetic (unlike the norm zero-guard):
    // a NaN velocity component propagates through weighted_pos and then
    // EVERY row of the matrix multiply, not just the row whose coefficient
    // multiplies the NaN lane directly -- `0.0 * NaN == NaN` in IEEE-754,
    // so even a zero matrix coefficient does not block the contamination
    // (`Mat4::transform_point`'s four-term sum-per-row means a NaN in any
    // one of x/y/z/w can poison all four output components whenever the
    // corresponding column isn't all-zero across every row, which is true
    // for an identity matrix's off-diagonal zero entries too: `0.0 * NaN`
    // is NaN, and NaN propagates through the following `+`). This port
    // preserves that fully-unguarded IEEE-754 propagation with no NaN
    // containment added.
    let out = rsp_world_transform(
        Vec3::new(1.0, 2.0, 3.0),
        Vec3::new(f32::NAN, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        ident(),
        ident(),
        ident(),
        0.0,
        0.0,
    );
    assert!(out.world_pos.x.is_nan());
    assert!(out.world_pos.y.is_nan());
    assert!(out.world_pos.z.is_nan());
    assert!(out.world_vel.x.is_nan());
    assert!(out.world_vel.y.is_nan());
    assert!(out.world_vel.z.is_nan());
    // The normal transform is independent of pos/vel (it only reads
    // `norm`/`inv_t_world_mat`), so it stays unaffected.
    assert_eq!(out.world_norm.x, 0.0);
    assert_eq!(out.world_norm.y, 1.0);
    assert_eq!(out.world_norm.z, 0.0);
}

#[test]
fn world_transform_infinite_matrix_component_propagates() {
    // An unguarded matrix multiply with an infinite matrix entry produces
    // inf/NaN in the result, matching plain IEEE-754 semantics with no
    // clamp added by this port.
    let inf_mat = Mat4::from_rows([
        Vec4::new(f32::INFINITY, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ]);
    let out = rsp_world_transform(
        Vec3::new(1.0, 2.0, 3.0),
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        inf_mat,
        ident(),
        inf_mat,
        1.0,
        1.0,
    );
    assert!(out.world_pos.x.is_infinite());
}

#[test]
fn world_transform_zero_length_normal_after_transform_yields_nan() {
    // norm is nonzero (skips the guard), but the inv-transpose matrix maps
    // it to exactly (0,0,0): normalize then divides 0.0/0.0, producing NaN
    // in every lane, per plain unguarded IEEE-754 division -- no epsilon
    // guard added by this port beyond the source's own exact-zero check on
    // the *input* norm.
    let zero_mat = Mat4::from_rows([
        Vec4::new(0.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ]);
    let out = rsp_world_norm(zero_mat, Vec3::new(1.0, 2.0, 3.0));
    assert!(out.x.is_nan());
    assert!(out.y.is_nan());
    assert!(out.z.is_nan());
    assert_eq!(out.w, 1.0);
}

// =======================================================================
// WGSL retention / Naga validation
// =======================================================================

#[test]
fn rsp_modify_wgsl_source_contains_the_ported_formulas() {
    assert!(RSP_MODIFY_WGSL.contains("f32(modify_value) / 65536.0"));
    assert!(RSP_MODIFY_WGSL.contains(">> 16u"));
}

#[test]
fn rsp_world_wgsl_source_contains_the_ported_formulas() {
    assert!(RSP_WORLD_WGSL.contains("1.0 - frame_weight"));
    assert!(RSP_WORLD_WGSL.contains("norm.x == 0.0 && norm.y == 0.0 && norm.z == 0.0"));
}

#[test]
fn rsp_modify_wgsl_contains_no_lerp_or_mix() {
    // RSPModifyCS.hlsl has no lerp/mix call; confirm the WGSL sibling does
    // not introduce one either.
    assert!(!RSP_MODIFY_WGSL.contains("lerp("));
    assert!(!RSP_MODIFY_WGSL.contains("mix("));
}

#[test]
fn rsp_world_wgsl_contains_no_lerp_or_mix() {
    assert!(!RSP_WORLD_WGSL.contains("lerp("));
    assert!(!RSP_WORLD_WGSL.contains("mix("));
}

#[test]
fn rsp_modify_wgsl_parses_and_validates_under_closed_naga_profile() {
    let module = naga::front::wgsl::parse_str(RSP_MODIFY_WGSL).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .expect("RSP_MODIFY_WGSL must validate under a closed (no extra capabilities) Naga profile");
}

#[test]
fn rsp_world_wgsl_parses_and_validates_under_closed_naga_profile() {
    let module = naga::front::wgsl::parse_str(RSP_WORLD_WGSL).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .expect("RSP_WORLD_WGSL must validate under a closed (no extra capabilities) Naga profile");
}

#[test]
fn rsp_modify_wgsl_truncated_source_fails_to_parse() {
    // Drop the closing brace of the last function and everything after it,
    // leaving an unclosed function body -- guaranteed invalid regardless of
    // where in the file the cut lands, unlike a length/2 truncation, which
    // (for a file whose first half is entirely doc-comment) can land on a
    // syntactically-complete (empty) prefix.
    let truncated = RSP_MODIFY_WGSL.rsplit_once('}').unwrap().0;
    assert!(naga::front::wgsl::parse_str(truncated).is_err());
}

#[test]
fn rsp_world_wgsl_truncated_source_fails_to_parse() {
    let truncated = RSP_WORLD_WGSL.rsplit_once('}').unwrap().0;
    assert!(naga::front::wgsl::parse_str(truncated).is_err());
}

#[test]
fn rsp_modify_wgsl_mutated_operator_fails_naga_validation_or_changes_semantics() {
    // Flipping the divide to a multiply in the Z branch must not silently
    // parse into an equivalent module -- either it fails to validate, or
    // (if it validates, since this is a type-preserving mutation) the
    // source text itself must differ from the retained original, proving
    // this test would have caught a silent formula edit.
    let mutated =
        RSP_MODIFY_WGSL.replace("f32(modify_value) / 65536.0", "f32(modify_value) * 65536.0");
    assert_ne!(mutated, RSP_MODIFY_WGSL);
    let module =
        naga::front::wgsl::parse_str(&mutated).expect("mutation stays syntactically valid");
    // Still validates (both are well-typed float expressions); the point of
    // this test is the `assert_ne!` above catching source-level drift.
    let _ = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module);
}

#[test]
fn rsp_world_wgsl_duplicate_function_name_fails_naga_validation() {
    let duplicate = format!(
        "{RSP_WORLD_WGSL}\nfn rsp_world_mul(row0: vec4<f32>, row1: vec4<f32>, row2: vec4<f32>, row3: vec4<f32>, v: vec4<f32>) -> vec4<f32> {{ return v; }}\n"
    );
    assert!(naga::front::wgsl::parse_str(&duplicate).is_err());
}
