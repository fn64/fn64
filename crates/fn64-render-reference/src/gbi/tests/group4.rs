// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

#![allow(
    clippy::excessive_precision,
    clippy::identity_op,
    clippy::needless_range_loop
)]
use super::support::*;
use crate::gbi::*;
use crate::gbi::wire::*;
use crate::gbi::types::*;
use crate::gbi::matrix::*;
use crate::gbi::tmem::*;
use crate::gbi::state::*;
use crate::gbi::entries::*;
use crate::gbi::stream::*;
use crate::gbi::geometry::*;
use fn64_render::{
    GeometryUcodeProfile, MicrocodeDataImageIdentity, RenderError, TaskAdmissionGeneration,
    TaskAdmissionSource, UcodeId,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt::Write as _};
use fn64_render::{F3dzex2Variant, TaskAdmissionUcode};

#[test]
fn light_vertex_modelview_rotates_light_into_local_space() {
    // computeDirLight brings the light dir into local space via the
    // modelview. With a 90° rotation about Y, a light along +X ends up
    // along the axis a +Z-facing normal is lit by. Concretely: rotate the
    // world +X light so it aligns with the vertex normal's frame, giving
    // full N·L where an unrotated dot would give 0.
    let mut st = lit_state();
    st.lights.num_dir = 1;
    st.lights.ambient = [0.0, 0.0, 0.0];
    st.lights.dir[0] = DirLight {
        dir: [1.0, 0.0, 0.0], // light along world +X
        col: [1.0, 1.0, 1.0],
    };
    // modelview that rotates +X -> +Z under rotate_dir (row-major,
    // column-vector): out.z = m[2][0]*x. Set m[2][0]=1, m[0][0]=0.
    let mut mv = identity();
    mv[0][0] = 0.0;
    mv[2][0] = 1.0;
    mv[2][2] = 0.0;
    st.modelview = mv;
    // Normal along +Z now sees the rotated light head-on.
    let c = light_vertex(&st, [0.0, 0.0, 1.0]);
    assert_eq!(c, [255, 255, 255]);
    // Sanity: WITHOUT the rotation (identity), the +X light and +Z normal
    // are orthogonal -> no diffuse.
    st.modelview = identity();
    let c0 = light_vertex(&st, [0.0, 0.0, 1.0]);
    assert_eq!(c0, [0, 0, 0]);
}

// --- Near-plane culling (the "fan from a point" fix) ----------------


#[test]
fn behind_near_plane_flags_nonpositive_w() {
    assert!(behind_near_plane(&vtx_w(-1.0)), "w<0 is behind camera");
    assert!(
        behind_near_plane(&vtx_w(0.0)),
        "w==0 is on the camera plane"
    );
    assert!(!behind_near_plane(&vtx_w(1.0)), "w>0 is in front");
}


#[test]
fn resolve_tri_drops_triangle_with_a_behind_camera_vertex() {
    // Fail-against-bug: a triangle with one vertex at w<=0 is the "fan
    // from a point" artifact (projecting it flings it across the screen).
    // resolve_tri must DROP it, not emit a giant wrong-side polygon.
    let mut cache = [Vertex::default(); 64];
    cache[0] = vtx_w(1.0);
    cache[1] = vtx_w(1.0);
    cache[2] = vtx_w(-0.5); // behind the near plane
    assert!(
        resolve_tri(
            &cache,
            [0, 1, 2],
            CullMode::None,
            None,
            OtherMode::default(),
            CombinerState::default(),
            BlenderState::default(),
        )
        .is_none(),
        "triangle touching a behind-camera vertex must be dropped"
    );
    // All three in front -> kept.
    cache[2] = vtx_w(2.0);
    assert!(resolve_tri(
        &cache,
        [0, 1, 2],
        CullMode::None,
        None,
        OtherMode::default(),
        CombinerState::default(),
        BlenderState::default(),
    )
    .is_some());
}

// --- Texture sampling (priority 4) ----------------------------------


#[test]
fn yuyv_pairs_decode_to_shared_chroma_and_distinct_luma() {
    let mut rdram = vec![0u8; 0x200];
    for (index, value) in [16, 128, 235, 128].into_iter().enumerate() {
        wr_u8(&mut rdram, 0x100 + index, value);
    }
    let mut tex = TexState {
        timg_addr: 0x100,
        timg_width: 2,
        ..TexState::default()
    };
    tex.tiles[0].fmt = G_IM_FMT_YUV;
    tex.tiles[0].siz = G_IM_SIZ_16B;
    tex.tiles[0].lrs = 4;
    let texture = decode_current_texture(&rdram, &tex, &[0; 16], 0, TextureLoad::Block);
    assert_eq!(&texture.texels[..4], &[16, 128, 128, 255]);
    assert_eq!(&texture.texels[4..8], &[235, 128, 128, 255]);
}


#[test]
#[should_panic(expected = "texture tile 0 uses unsupported format 1 size 1")]
fn direct_texture_oracle_traps_instead_of_falling_back_to_flat_shading() {
    let mut tex = TexState::default();
    tex.tiles[0].fmt = G_IM_FMT_YUV;
    tex.tiles[0].siz = G_IM_SIZ_8B;
    let _ = decode_current_texture(&[0; 16], &tex, &[0; 16], 0, TextureLoad::Block);
}


#[test]
fn texture_conversion_modes_execute_point_filter_and_filter_convert() {
    let convert = ConvertState::default();
    assert_eq!(
        convert.convert_texel([100, 128, 128, 255]),
        [100, 100, 100, 255]
    );
    assert_eq!(
        convert.convert_texel([100, 255, 255, 255]),
        [255, 0, 255, 255]
    );

    let texture = Texture {
        format: G_IM_FMT_YUV,
        size: G_IM_SIZ_16B,
        width: 2,
        height: 1,
        texels: std::rc::Rc::new(vec![20, 128, 128, 255, 220, 128, 128, 255]),
        clamp_s: true,
        clamp_t: true,
        mirror_s: false,
        mirror_t: false,
        mask_s: 0,
        mask_t: 0,
        shift_s: 0,
        shift_t: 0,
        origin_s: 0.0,
        origin_t: 0.0,
        tmem: None,
        lod: None,
    };
    let base = OtherMode::default().raw_high() & !(7 << 9);
    let conv = OtherMode::from_raw(base, 0, 0);
    let filtconv = OtherMode::from_raw(base | (5 << 9) | (3 << 12), 0, 0);
    let filt = OtherMode::from_raw(base | (6 << 9) | (3 << 12), 0, 0);
    assert_eq!(texture.sample_rdp(0.75, 0.0, conv, convert)[0], 20);
    assert_eq!(texture.sample_rdp(0.5, 0.0, filtconv, convert)[0], 120);
    assert_eq!(texture.sample_rdp(0.5, 0.0, filt, convert)[0], 120);
}


#[test]
fn lod_selection_matches_public_mip_detail_and_sharpen_tables() {
    let snapshot = TextureLodSnapshot {
        tiles: std::array::from_fn(|_| None),
        primitive_tile: 2,
        max_level: 3,
    };
    let base = OtherMode::default().raw_high() & !((1 << 16) | (3 << 17));
    let mode = |detail: u32| OtherMode::from_raw(base | (1 << 16) | (detail << 17), 0, 0);
    let derivatives = |lod: f32| TextureDerivatives {
        dsdx: lod,
        ..TextureDerivatives::default()
    };

    assert_eq!(
        Texture::lod_selection(&snapshot, derivatives(7.5), mode(0), 0),
        TextureLodSelection {
            tile0: 4,
            tile1: 5,
            fraction: 0.875,
        }
    );
    assert_eq!(
        Texture::lod_selection(&snapshot, derivatives(0.25), mode(0), 0),
        TextureLodSelection {
            tile0: 2,
            tile1: 2,
            fraction: 0.25,
        }
    );
    let detail = Texture::lod_selection(&snapshot, derivatives(0.25), mode(2), 128);
    assert_eq!((detail.tile0, detail.tile1), (2, 3));
    assert!((detail.fraction - 128.0 / 255.0).abs() < f32::EPSILON);
    assert_eq!(
        Texture::lod_selection(&snapshot, derivatives(0.5), mode(1), 0),
        TextureLodSelection {
            tile0: 2,
            tile1: 3,
            fraction: -0.5,
        }
    );
    assert_eq!(
        Texture::lod_selection(&snapshot, derivatives(2.5), mode(2), 0),
        TextureLodSelection {
            tile0: 4,
            tile1: 5,
            fraction: 0.25,
        }
    );
}


#[test]
#[should_panic(expected = "RDP combiner selected TEXEL1 without a decoded tile+1 image")]
fn missing_texel1_never_aliases_texel0() {
    checker_2x2(true).sample_rdp_pair(
        None,
        TextureSampleRequest {
            s: 0.0,
            t: 0.0,
            derivatives: TextureDerivatives::default(),
            other_mode: OtherMode::default(),
            convert: ConvertState::default(),
            min_level: 0,
            require_texel1: true,
        },
    );
}


#[test]
fn unused_texel1_does_not_sample_the_adjacent_tile() {
    let tile0 = checker_2x2(true);
    let mut invalid_tile1 = checker_2x2(true);
    invalid_tile1.texels = std::rc::Rc::new(Vec::new());
    let mut tiles = std::array::from_fn(|_| None);
    tiles[0] = Some(tile0.clone());
    tiles[1] = Some(invalid_tile1);
    let texture = tile0.with_lod_snapshot(tiles, 0, 0);

    let (texel0, texel1, fraction) = texture.sample_rdp_pair(
        None,
        TextureSampleRequest {
            s: 0.0,
            t: 0.0,
            derivatives: TextureDerivatives::default(),
            other_mode: OtherMode::default(),
            convert: ConvertState::default(),
            min_level: 0,
            require_texel1: false,
        },
    );
    assert_eq!(texel1, texel0);
    assert_eq!(fraction, 0.0);
}


#[test]
#[should_panic(
    expected = "RDP LOD selected tile 1 without a decoded G_LOADBLOCK/G_LOADTILE image"
)]
fn missing_lod_selected_tile_traps_by_index() {
    let tile0 = checker_2x2(true);
    let mut tiles = std::array::from_fn(|_| None);
    tiles[0] = Some(tile0.clone());
    let texture = tile0.with_lod_snapshot(tiles, 0, 2);
    let high = (OtherMode::default().raw_high() & !(1 << 16)) | (1 << 16);
    texture.sample_rdp_pair(
        None,
        TextureSampleRequest {
            s: 0.0,
            t: 0.0,
            derivatives: TextureDerivatives {
                dsdx: 2.5,
                ..TextureDerivatives::default()
            },
            other_mode: OtherMode::from_raw(high, 0, 0),
            convert: ConvertState::default(),
            min_level: 0,
            require_texel1: false,
        },
    );
}


#[test]
fn texture_and_primitive_commands_retain_lod_limits() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_TEXTURE as u32) << 24) | (5 << 11) | (3 << 8) | 2,
        0xffff_ffff,
    );
    wr_cmd(
        &mut rdram,
        0x1008,
        ((G_SETPRIMCOLOR as u32) << 24) | (0x80 << 8) | 0x40,
        0x0102_0304,
    );
    wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);

    let state = decode_display_list_f3dex2_state(&rdram, 0x1000).unwrap();
    assert!(state.tex.tex_enabled);
    assert_eq!((state.tex.tex_tile, state.tex.tex_max_level), (3, 5));
    assert_eq!(state.combiner.min_lod_level, 0x80);
    assert_eq!(state.combiner.prim_lod_fraction, 0x40);
}


#[test]
fn texture_samples_the_right_texel() {
    let tex = checker_2x2(true);
    // Each integer texel coordinate lands on its own texel (nearest).
    assert_eq!(tex.sample(0.0, 0.0), [255, 0, 0, 255]); // TL red
    assert_eq!(tex.sample(1.0, 0.0), [0, 255, 0, 255]); // TR green
    assert_eq!(tex.sample(0.0, 1.0), [0, 0, 255, 255]); // BL blue
    assert_eq!(tex.sample(1.0, 1.0), [255, 255, 255, 255]); // BR white

    // Fractional coords floor to the containing texel.
    assert_eq!(tex.sample(0.9, 0.1), [255, 0, 0, 255]); // floor -> (0,0) red
}


#[test]
fn texture_sample_floor_addressing() {
    let tex = checker_2x2(true);
    // (1.5, 0.9) floors to (1, 0) = green.
    assert_eq!(tex.sample(1.5, 0.9), [0, 255, 0, 255]);
    // (0.2, 1.7) floors to (0, 1) = blue.
    assert_eq!(tex.sample(0.2, 1.7), [0, 0, 255, 255]);
}


#[test]
fn texture_filter_matches_public_point_average_and_triangular_rules() {
    let texture = Texture {
        format: 0,
        size: 2,
        width: 2,
        height: 2,
        texels: std::rc::Rc::new(vec![
            0, 0, 0, 0, // c00
            100, 100, 100, 100, // c10
            200, 200, 200, 200, // c01
            255, 255, 255, 255, // c11
        ]),
        clamp_s: true,
        clamp_t: true,
        mirror_s: false,
        mirror_t: false,
        mask_s: 0,
        mask_t: 0,
        shift_s: 0,
        shift_t: 0,
        origin_s: 0.0,
        origin_t: 0.0,
        tmem: None,
        lod: None,
    };

    assert_eq!(
        texture.sample_filtered(0.75, 0.75, TextureFilter::Point),
        [0; 4]
    );
    assert_eq!(
        texture.sample_filtered(0.5, 0.5, TextureFilter::Average),
        [139; 4]
    );
    assert_eq!(
        texture.sample_filtered(0.25, 0.25, TextureFilter::Bilinear),
        [75; 4],
        "upper triangle must interpolate c00/c10/c01"
    );
    assert_eq!(
        texture.sample_filtered(0.75, 0.75, TextureFilter::Bilinear),
        [203; 4],
        "lower triangle must interpolate c11/c01/c10"
    );
}


#[test]
fn texture_s10_5_coordinate_sweep_covers_every_grid_value_and_tile_shift() {
    for raw in i16::MIN..=i16::MAX {
        let coordinate = TextureCoordinateS10_5::from_texels_bounded(f32::from(raw) / 32.0);
        assert_eq!(coordinate.0, raw);
        assert_eq!(coordinate.shifted(0).texel(), i64::from(raw).div_euclid(32));
        assert_eq!(
            coordinate.shifted(0).fraction(),
            i64::from(raw).rem_euclid(32)
        );
        for encoded in 0..=15 {
            let expected = match encoded {
                0 => i64::from(raw),
                1..=10 => i64::from(raw) >> encoded,
                11..=15 => i64::from(raw) * (1_i64 << (16 - encoded)),
                _ => unreachable!(),
            };
            assert_eq!(
                coordinate.shifted(encoded).0,
                expected,
                "raw={raw} shift={encoded}"
            );
        }
    }

    // A non-finite coordinate has no valid register value and still traps.
    assert!(std::panic::catch_unwind(|| {
        TextureCoordinateS10_5::from_texels_bounded(f32::NAN)
    })
    .is_err());
    // Finite coordinates outside the nominal -1024..+1023.99 window do NOT
    // trap: the hardware coordinate register is fixed-width and overflows
    // modularly (wrapped/tiled surfaces address beyond the window every
    // frame), with the per-tile clamp/mirror/wrap addressing resolving the
    // final texel. Verify the modular wrap into the S10.5 register width.
    // 1024.0 texels == 0x8000 in S10.5 -> wraps to i16::MIN.
    assert_eq!(
        TextureCoordinateS10_5::from_texels_bounded(1024.0).0,
        i16::MIN
    );
    // Just below -1024 wraps one 1/32-texel cell down from i16::MIN, i.e.
    // to i16::MAX.
    assert_eq!(
        TextureCoordinateS10_5::from_texels_bounded(-1024.0 - 1.0 / 32.0).0,
        i16::MAX
    );
    // The window endpoints themselves are exact and unchanged.
    assert_eq!(
        TextureCoordinateS10_5::from_texels_bounded(-1024.0).0,
        i16::MIN
    );
    assert_eq!(
        TextureCoordinateS10_5::from_texels_bounded(1024.0 - 1.0 / 32.0).0,
        i16::MAX
    );
}


#[test]
fn texture_fixed_s10_5_filter_sweeps_both_triangle_halves_without_float_drift() {
    let mut lower_half = 0usize;
    let mut upper_half = 0usize;
    for seed in 0..=255u16 {
        let values = [
            seed as u8,
            seed.wrapping_mul(73).wrapping_add(19) as u8,
            seed.wrapping_mul(151).wrapping_add(41) as u8,
            seed.wrapping_mul(211).wrapping_add(97) as u8,
        ];
        let samples = values.map(|value| [value; 4]);
        for sf in 0..32i64 {
            for tf in 0..32i64 {
                let [c00, c10, c01, c11] = values.map(f32::from);
                let sf_float = sf as f32 / 32.0;
                let tf_float = tf as f32 / 32.0;
                let expected = if sf + tf <= 32 {
                    lower_half += 1;
                    c00 + sf_float * (c10 - c00) + tf_float * (c01 - c00)
                } else {
                    upper_half += 1;
                    c11 + (1.0 - sf_float) * (c01 - c11) + (1.0 - tf_float) * (c10 - c11)
                }
                .round()
                .clamp(0.0, 255.0) as u8;
                assert_eq!(
                    filter_three_nearest_s10_5(samples, sf, tf),
                    [expected; 4],
                    "seed={seed} sf={sf}/32 tf={tf}/32"
                );
            }
        }
    }
    assert_eq!((lower_half, upper_half), (143_104, 119_040));
}


#[test]
fn texture_fixed_s10_5_negative_half_texel_observes_wrap_mirror_and_clamp_boundaries() {
    let mut texture = indexed_texture(4);
    texture.mask_s = 2;
    texture.clamp_s = false;
    texture.texels = std::rc::Rc::new(
        [0u8, 64, 128, 255]
            .into_iter()
            .flat_map(|value| [value; 4])
            .collect(),
    );

    assert_eq!(texture.sample(-1.0 / 32.0, 0.0), [255; 4]);
    assert_eq!(
        texture.sample_filtered(1.0 / 64.0, 0.0, TextureFilter::Bilinear),
        [0; 4],
        "bounded host conversion floors a positive sub-grid fraction to S10.5 zero"
    );
    assert_eq!(
        texture.sample_filtered(-1.0 / 64.0, 0.0, TextureFilter::Bilinear),
        [8; 4],
        "bounded host conversion floors a negative sub-grid fraction to -1/32"
    );
    assert_eq!(
        texture.sample_filtered(-0.5, 0.0, TextureFilter::Bilinear),
        [128; 4],
        "wrapped -1/2 selects texels 3 and 0 at equal S10.5 weights"
    );
    texture.mirror_s = true;
    assert_eq!(texture.sample(-1.0 / 32.0, 0.0), [0; 4]);
    texture.clamp_s = true;
    assert_eq!(
        texture.sample_filtered(-0.5, 0.0, TextureFilter::Bilinear),
        [0; 4]
    );

    for raw in -96..=96i16 {
        let coordinate = TextureCoordinateS10_5(raw);
        for shift in 0..=15 {
            let shifted = coordinate.shifted(shift).texel();
            for mask in 0..=15 {
                for clamp in [false, true] {
                    for mirror in [false, true] {
                        let clamped = if mask == 0 || clamp {
                            shifted.clamp(0, 36)
                        } else {
                            shifted
                        };
                        let expected = if mask == 0 {
                            clamped as u32
                        } else {
                            let low_mask = (1_i64 << mask) - 1;
                            if mirror && clamped & (1_i64 << mask) != 0 {
                                ((!clamped) & low_mask) as u32
                            } else {
                                (clamped & low_mask) as u32
                            }
                        };
                        assert_eq!(
                            texture_axis_address(
                                shifted,
                                37,
                                clamp,
                                mirror,
                                mask,
                                TextureAddressMode::Programmed,
                            ),
                            expected,
                            "raw={raw} shift={shift} mask={mask} clamp={clamp} mirror={mirror}"
                        );
                    }
                }
            }
        }
    }
}


#[test]
fn texture_shift_precedes_fractional_tile_origin_subtraction() {
    let mut texture = indexed_texture(8);
    texture.clamp_s = true;
    texture.origin_s = 0.5;

    texture.shift_s = 1;
    assert_eq!(
        texture.sample(6.5, 0.0)[0],
        2,
        "right shift first gives 6.5/2-0.5=2.75; subtract-first would select 3"
    );

    texture.shift_s = 15;
    assert_eq!(
        texture.sample(1.25, 0.0)[0],
        2,
        "left shift first gives 1.25*2-0.5=2; subtract-first would select 1"
    );
}


#[test]
fn texture_clamp_vs_wrap_addressing() {
    let clamp = checker_2x2(true);
    // Out-of-range clamps to the edge texel.
    assert_eq!(clamp.sample(5.0, 0.0), [0, 255, 0, 255]); // clamp to x=1 green
    assert_eq!(clamp.sample(-3.0, 1.0), [0, 0, 255, 255]); // clamp to x=0 blue

    let wrap = checker_2x2(false);
    // Wrap repeats: s=2 -> texel 0, s=3 -> texel 1, s=-1 -> texel 1.
    assert_eq!(wrap.sample(2.0, 0.0), [255, 0, 0, 255]); // (0,0) red
    assert_eq!(wrap.sample(3.0, 0.0), [0, 255, 0, 255]); // (1,0) green
    assert_eq!(wrap.sample(-1.0, 0.0), [0, 255, 0, 255]); // wraps to (1,0)
}


#[test]
fn copy_clamp_axis_sweep_matches_public_bypass_then_wrap_mirror_equation() {
    const DIMENSION: u32 = 37;
    for mode in [TextureAddressMode::Programmed, TextureAddressMode::Copy] {
        for mask in 0..=15u8 {
            for clamp in [false, true] {
                for mirror in [false, true] {
                    for input in -1024..=1023i64 {
                        let clamps =
                            mode == TextureAddressMode::Programmed && (mask == 0 || clamp);
                        let coordinate = if clamps {
                            input.clamp(0, i64::from(DIMENSION) - 1)
                        } else {
                            input
                        };
                        let expected = if mask == 0 {
                            coordinate as u32
                        } else {
                            let low_mask = (1_i64 << mask) - 1;
                            if mirror && coordinate & (1_i64 << mask) != 0 {
                                ((!coordinate) & low_mask) as u32
                            } else {
                                (coordinate & low_mask) as u32
                            }
                        };
                        assert_eq!(
                            texture_axis_address(input, DIMENSION, clamp, mirror, mask, mode,),
                            expected,
                            "mode={mode:?} coordinate={input} mask={mask} clamp={clamp} mirror={mirror}"
                        );
                    }
                }
            }
        }
    }
    assert_eq!(
        texture_axis_address(99, 0, true, true, 15, TextureAddressMode::Copy),
        0
    );
}


#[test]
fn texture_mask_mirror_and_clamp_follow_public_coordinate_sequences() {
    let mut texture = indexed_texture(4);
    texture.mask_s = 2;
    assert_eq!(
        (0..8)
            .map(|s| texture.sample(s as f32, 0.0)[0])
            .collect::<Vec<_>>(),
        [0, 1, 2, 3, 0, 1, 2, 3]
    );

    texture.mirror_s = true;
    assert_eq!(
        (0..8)
            .map(|s| texture.sample(s as f32, 0.0)[0])
            .collect::<Vec<_>>(),
        [0, 1, 2, 3, 3, 2, 1, 0],
        "Programming Manual Chapter 13 mask=2 mirror sequence"
    );

    let mut clamp_after_one_mirror = indexed_texture(12);
    clamp_after_one_mirror.mask_s = 2;
    clamp_after_one_mirror.mirror_s = true;
    clamp_after_one_mirror.clamp_s = true;
    assert_eq!(
        (8..16)
            .map(|s| clamp_after_one_mirror.sample(s as f32, 0.0)[0])
            .collect::<Vec<_>>(),
        [0, 1, 2, 3, 3, 3, 3, 3],
        "clamp must freeze the input at SH before mirror/mask addressing"
    );
}


#[test]
fn texture_shift_decodes_public_right_and_left_ranges() {
    let mut texture = indexed_texture(64);
    texture.clamp_s = true;
    texture.shift_s = 1;
    assert_eq!(texture.sample(6.0, 0.0)[0], 3);
    texture.shift_s = 11;
    assert_eq!(texture.sample(1.0, 0.0)[0], 32);
    texture.shift_s = 15;
    assert_eq!(texture.sample(3.0, 0.0)[0], 6);
}


#[test]
fn set_tile_retains_all_public_address_fields_and_existing_extent() {
    let mut tile = Tile {
        uls: 4,
        ult: 8,
        lrs: 12,
        lrt: 16,
        ..Default::default()
    };
    let w0 = (G_IM_FMT_CI as u32) << 21 | (G_IM_SIZ_8B as u32) << 19 | 0x155 << 9 | 0x12a;
    let w1 = 7 << 24 | 9 << 20 | 3 << 18 | 5 << 14 | 12 << 10 | 1 << 8 | 4 << 4 | 15;
    apply_set_tile(&mut tile, w0, w1);

    assert_eq!(tile.fmt, G_IM_FMT_CI);
    assert_eq!(tile.siz, G_IM_SIZ_8B);
    assert_eq!(tile.line, 0x155);
    assert_eq!(tile.tmem, 0x12a);
    assert_eq!(tile.palette, 9);
    assert!(tile.clamp_t && tile.mirror_t);
    assert!(!tile.clamp_s && tile.mirror_s);
    assert_eq!((tile.mask_s, tile.mask_t), (4, 5));
    assert_eq!((tile.shift_s, tile.shift_t), (15, 12));
    assert_eq!((tile.uls, tile.ult, tile.lrs, tile.lrt), (4, 8, 12, 16));
}


#[test]
fn rgba5551_expands_high_bits() {
    // Pure red (R5=0x1F) -> R8=0xFF; alpha bit set -> 0xFF.
    assert_eq!(rgba5551_to_rgba8888(0xF801), [255, 0, 0, 255]);
    // Pure green (G5=0x1F at bits 6..10).
    assert_eq!(rgba5551_to_rgba8888(0x07C1), [0, 255, 0, 255]);
    // Black, alpha 0.
    assert_eq!(rgba5551_to_rgba8888(0x0000), [0, 0, 0, 0]);
}


#[test]
fn color_image_layout_classifies_exactly_the_public_memory_interfaces() {
    let image = |format, size| ColorImage {
        format,
        size,
        width: 1,
        address: 0,
    };
    assert_eq!(
        image(ColorImage::CI_FORMAT, ColorImage::BITS_8).layout(),
        Some(ColorImageLayout::Index8)
    );
    assert_eq!(
        image(4, ColorImage::BITS_8).layout(),
        Some(ColorImageLayout::Index8),
        "the public 8-bit memory interface is selected by size"
    );
    assert_eq!(
        image(ColorImage::RGBA_FORMAT, ColorImage::BITS_16).layout(),
        Some(ColorImageLayout::Rgba16)
    );
    assert_eq!(
        image(ColorImage::RGBA_FORMAT, ColorImage::BITS_32).layout(),
        Some(ColorImageLayout::Rgba32)
    );
    for (format, size) in [(1, 2), (2, 2), (3, 3), (0, 0)] {
        assert_eq!(
            image(format, size).layout(),
            None,
            "format={format} size={size}"
        );
    }
}


#[test]
fn color_image_transition_matrix_admits_every_public_pair() {
    let image = |layout| ColorImage {
        format: match layout {
            ColorImageLayout::Index8 => ColorImage::CI_FORMAT,
            ColorImageLayout::Rgba16 | ColorImageLayout::Rgba32 => ColorImage::RGBA_FORMAT,
        },
        size: match layout {
            ColorImageLayout::Index8 => ColorImage::BITS_8,
            ColorImageLayout::Rgba16 => ColorImage::BITS_16,
            ColorImageLayout::Rgba32 => ColorImage::BITS_32,
        },
        width: 4,
        address: 0,
    };

    for from in ColorImageLayout::ALL {
        for to in ColorImageLayout::ALL {
            assert_eq!(
                image(from).transition_to(image(to)),
                ColorImageLayoutTransition { from, to }
            );
        }
    }
}


#[test]
#[should_panic(expected = "unsupported destination color-image layout")]
fn color_image_transition_traps_an_unsupported_destination() {
    ColorImage {
        format: ColorImage::RGBA_FORMAT,
        size: ColorImage::BITS_16,
        width: 1,
        address: 0,
    }
    .transition_to(ColorImage {
        format: ColorImage::CI_FORMAT,
        size: ColorImage::BITS_16,
        width: 1,
        address: 0,
    });
}


#[test]
fn load_tlut_count_uses_all_ten_wire_bits() {
    // Public gbi.h encodes `count - 1` directly, without quarter-texel
    // scaling. Discarding the low two bits turns the normal 256-entry CI8
    // palette into 64 entries.
    assert_eq!(load_tlut_count(255 << 14), 256);
    assert_eq!(load_tlut_count(15 << 14), 16);
}


#[test]
fn tmem_storage_matches_public_odd_row_and_rgba32_bank_layouts() {
    let mut storage = Tmem::default();
    let rgba16 = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_16B,
        line: 1,
        ..Default::default()
    };
    storage.write_texel(rgba16, 0, 0, false, G_IM_SIZ_16B, 0x1122);
    storage.write_texel(rgba16, 0, 1, true, G_IM_SIZ_16B, 0x3344);
    assert_eq!(&storage.bytes[0..2], &[0x11, 0x22]);
    // Row 1 logical byte 8 is exchanged into the upper 32-bit long.
    assert_eq!(&storage.bytes[12..14], &[0x33, 0x44]);
    assert_eq!(storage.read_texel(rgba16, 0, 0, G_IM_SIZ_16B), 0x1122);
    assert_eq!(storage.read_texel(rgba16, 0, 1, G_IM_SIZ_16B), 0x3344);

    let mut storage = Tmem::default();
    let rgba32 = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_32B,
        line: 1,
        ..Default::default()
    };
    storage.write_texel(rgba32, 0, 0, false, G_IM_SIZ_32B, 0x1122_3344);
    storage.write_texel(rgba32, 0, 1, true, G_IM_SIZ_32B, 0x5566_7788);
    assert_eq!(&storage.bytes[0..2], &[0x11, 0x22]);
    assert_eq!(
        &storage.bytes[TMEM_HALF_BYTES..TMEM_HALF_BYTES + 2],
        &[0x33, 0x44]
    );
    assert_eq!(&storage.bytes[12..14], &[0x55, 0x66]);
    assert_eq!(
        &storage.bytes[TMEM_HALF_BYTES + 12..TMEM_HALF_BYTES + 14],
        &[0x77, 0x88]
    );
    assert_eq!(storage.read_texel(rgba32, 0, 0, G_IM_SIZ_32B), 0x1122_3344);
    assert_eq!(storage.read_texel(rgba32, 0, 1, G_IM_SIZ_32B), 0x5566_7788);

    let mut storage = Tmem::default();
    let i4 = Tile {
        fmt: G_IM_FMT_I,
        siz: G_IM_SIZ_4B,
        line: 1,
        ..Default::default()
    };
    for x in 0..16 {
        storage.write_texel(i4, x, 1, true, G_IM_SIZ_4B, x as u32);
    }
    // On an odd row, texels 0..7 occupy the second 32-bit long and
    // texels 8..15 occupy the first, as in Manual Figure 13.8.3.
    assert_eq!(&storage.bytes[8..12], &[0x89, 0xab, 0xcd, 0xef]);
    assert_eq!(&storage.bytes[12..16], &[0x01, 0x23, 0x45, 0x67]);
}


// The read_byte diagnostic context is a lazy `FnOnce() -> String`, so on a
// valid TMEM read (the per-texel hot path) the context is never formatted.
#[test]
fn read_byte_context_closure_is_not_evaluated_on_a_valid_read() {
    let mut storage = Tmem::default();
    let tile = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_16B,
        line: 1,
        ..Default::default()
    };
    // Initialize the byte so the read is valid.
    storage.write_texel(tile, 0, 0, false, G_IM_SIZ_16B, 0x1122);
    let evaluated = std::cell::Cell::new(false);
    let byte = storage.read_byte(0, false, 0xff, || {
        evaluated.set(true);
        "should-not-be-built".to_string()
    });
    assert_eq!(byte, 0x11);
    assert!(
        !evaluated.get(),
        "diagnostic context must not be formatted on a valid read"
    );
}


#[test]
fn lazy_context_preserves_exact_uninitialized_read_diagnostics() {
    let storage = Tmem::default();
    assert_eq!(
        panic_text(|| {
            let _ = storage.read_byte(0, false, 0xff, || "diag-marker-42".to_owned());
        }),
        "assertion `left == right` failed: diag-marker-42 reads uninitialized TMEM bits at byte 0x000\n  left: 0\n right: 255"
    );

    let tile = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_16B,
        line: 1,
        ..Default::default()
    };
    assert_eq!(
        panic_text(|| {
            let _ = storage.read_texel(tile, 3, 0, G_IM_SIZ_16B);
        }),
        "assertion `left == right` failed: tile at TMEM word 0 texel (3, 0) reads uninitialized TMEM bits at byte 0x006\n  left: 0\n right: 255"
    );
    assert_eq!(
        panic_text(|| {
            let _ = storage.read_tlut(7, 2);
        }),
        "assertion `left == right` failed: TLUT index 7 reads uninitialized TMEM bits at byte 0x838\n  left: 0\n right: 255"
    );

    let yuv = TmemTexture {
        storage: std::rc::Rc::new(Tmem::default()),
        tile: Tile {
            fmt: G_IM_FMT_YUV,
            siz: G_IM_SIZ_16B,
            line: 1,
            tmem: 3,
            ..Default::default()
        },
        texture_lut: 0,
    };
    assert_eq!(
        panic_text(|| {
            let _ = yuv.sample(2, 4);
        }),
        "assertion `left == right` failed: YUV tile at TMEM word 3 texel (2, 4) reads uninitialized TMEM bits at byte 0x03a\n  left: 0\n right: 255"
    );
}


#[test]
fn lazy_context_preserves_all_read_texel_values() {
    let mut storage = Tmem::default();
    let i4 = Tile {
        fmt: G_IM_FMT_I,
        siz: G_IM_SIZ_4B,
        line: 1,
        ..Default::default()
    };
    storage.write_texel(i4, 0, 0, false, G_IM_SIZ_4B, 0x0a);
    storage.write_texel(i4, 1, 0, false, G_IM_SIZ_4B, 0x05);
    assert_eq!(storage.read_texel(i4, 0, 0, G_IM_SIZ_4B), 0x0a);
    assert_eq!(storage.read_texel(i4, 1, 0, G_IM_SIZ_4B), 0x05);

    let mut storage = Tmem::default();
    let i8 = Tile {
        fmt: G_IM_FMT_I,
        siz: G_IM_SIZ_8B,
        line: 1,
        ..Default::default()
    };
    storage.write_texel(i8, 0, 0, false, G_IM_SIZ_8B, 0xab);
    assert_eq!(storage.read_texel(i8, 0, 0, G_IM_SIZ_8B), 0xab);

    let mut storage = Tmem::default();
    let rgba16 = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_16B,
        line: 1,
        ..Default::default()
    };
    storage.write_texel(rgba16, 0, 0, false, G_IM_SIZ_16B, 0xBEEF);
    storage.write_texel(rgba16, 1, 0, false, G_IM_SIZ_16B, 0xF00D);
    assert_eq!(storage.read_texel(rgba16, 0, 0, G_IM_SIZ_16B), 0xBEEF);
    assert_eq!(storage.read_texel(rgba16, 1, 0, G_IM_SIZ_16B), 0xF00D);

    let mut storage = Tmem::default();
    let rgba32 = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_32B,
        line: 1,
        ..Default::default()
    };
    storage.write_texel(rgba32, 0, 0, false, G_IM_SIZ_32B, 0x0123_4567);
    assert_eq!(storage.read_texel(rgba32, 0, 0, G_IM_SIZ_32B), 0x0123_4567);
}


#[test]
fn yuv_tmem_splits_chroma_low_and_luma_high() {
    let mut storage = Tmem::default();
    let tile = Tile {
        fmt: G_IM_FMT_YUV,
        siz: G_IM_SIZ_16B,
        line: 1,
        ..Default::default()
    };
    storage.write_yuv_pair(tile, 0, 0, false, [0x10, 0x20, 0x30, 0x40]);
    assert_eq!(&storage.bytes[0..2], &[0x20, 0x40]);
    assert_eq!(
        &storage.bytes[TMEM_HALF_BYTES..TMEM_HALF_BYTES + 2],
        &[0x10, 0x30]
    );
    let texture = TmemTexture {
        storage: std::rc::Rc::new(storage),
        tile,
        texture_lut: 0,
    };
    assert_eq!(texture.sample(0, 0), [0x10, 0x20, 0x40, 255]);
    assert_eq!(texture.sample(1, 0), [0x30, 0x20, 0x40, 255]);
}


#[test]
fn load_and_render_tiles_share_snapshotted_tmem_beyond_extent() {
    let base = 0x100usize;
    let source = [0xf801u16, 0x07c1, 0x003f, 0xffff];
    let mut rdram = vec![0; base + 16];
    for (index, value) in source.into_iter().enumerate() {
        let [hi, lo] = value.to_be_bytes();
        wr_u8(&mut rdram, base + index * 2, hi);
        wr_u8(&mut rdram, base + index * 2 + 1, lo);
    }
    let mut tex = TexState {
        timg_addr: base as u32,
        timg_fmt: G_IM_FMT_RGBA,
        timg_siz: G_IM_SIZ_16B,
        timg_width: 4,
        ..Default::default()
    };
    tex.tiles[7] = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_16B,
        ..Default::default()
    };
    load_block_into_tmem(
        &rdram,
        &mut tex,
        &[0; 16],
        7,
        u32::from(G_LOADBLOCK) << 24,
        (7 << 24) | (3 << 12),
    );
    // A separate render tile reinterprets the same TMEM word. Its active
    // clamp extent is only two texels, but mask=2 deliberately addresses
    // all four loaded texels.
    tex.tiles[0] = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_16B,
        line: 1,
        mask_s: 2,
        lrs: 4,
        ..Default::default()
    };
    let texture = bind_texture_set(&tex, 0, 0, 0).expect("render tile must bind TMEM");
    assert_eq!(texture.sample(0.0, 0.0), [255, 0, 0, 255]);
    assert_eq!(texture.sample(3.0, 0.0), [255, 255, 255, 255]);

    std::rc::Rc::make_mut(&mut tex.tmem).write_texel(
        tex.tiles[0],
        0,
        0,
        false,
        G_IM_SIZ_16B,
        0x003f,
    );
    assert_eq!(
        texture.sample(0.0, 0.0),
        [255, 0, 0, 255],
        "a later TMEM load must not mutate an emitted primitive"
    );
    let reloaded = bind_texture_set(&tex, 0, 0, 0).expect("reloaded tile must bind");
    assert_eq!(reloaded.sample(0.0, 0.0), [0, 0, 255, 255]);
}


#[test]
fn wrapped_tile_accepts_an_origin_above_its_unused_clamp_bound() {
    let mut tex = TexState::default();
    tex.tiles[1] = Tile {
        fmt: G_IM_FMT_I,
        siz: G_IM_SIZ_8B,
        line: 1,
        mask_t: 5,
        ult: 0x0ffd,
        lrt: 0,
        ..Default::default()
    };
    std::rc::Rc::make_mut(&mut tex.tmem).write_texel(
        tex.tiles[1],
        0,
        0,
        true,
        G_IM_SIZ_8B,
        0x7f,
    );

    let texture = texture_for_tile(&tex, 1, 0, &tex.tmem)
        .expect("wrap mask, not unused clamp bounds, defines tile validity");
    assert_eq!(texture.height, 32);
    assert_eq!(texture.sample(0.0, 0.0), [0x7f; 4]);
}


#[test]
fn texel1_gap_reversed_clamp_extent_is_invalid_without_eager_unsigned_subtraction() {
    let mut tex = TexState::default();
    tex.tiles[1] = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_16B,
        line: 1,
        clamp_s: true,
        uls: 4,
        lrs: 0,
        ..Default::default()
    };
    std::rc::Rc::make_mut(&mut tex.tmem).write_texel(
        tex.tiles[1],
        0,
        0,
        false,
        G_IM_SIZ_16B,
        0xf801,
    );

    assert!(
        texture_for_tile(&tex, 1, 0, &tex.tmem).is_none(),
        "a reversed clamped extent is not a bindable tile"
    );
}


#[test]
fn ci4_samples_quadricated_tlut_at_palette_bank_address() {
    let mut storage = Tmem::default();
    let tile = Tile {
        fmt: G_IM_FMT_CI,
        siz: G_IM_SIZ_4B,
        palette: 2,
        line: 1,
        ..Default::default()
    };
    storage.write_texel(tile, 0, 0, false, G_IM_SIZ_4B, 1);
    storage.write_tlut(256, 0x21, 0xf801);
    let texture = TmemTexture {
        storage: std::rc::Rc::new(storage),
        tile,
        texture_lut: 2,
    };
    assert_eq!(texture.sample(0, 0), [255, 0, 0, 255]);
}


#[test]
fn tlut_mode_palettizes_eight_bit_texels_regardless_of_tile_format() {
    // EN_TLUT is a pipeline mode, not a tile-format property: WM2000's
    // title scene draws its full-screen CI8 logo image through a render
    // tile DECLARED IA8 with G_TT_RGBA16 enabled, and hardware still
    // palettizes every 8-bit texel through high TMEM. A previous
    // revision keyed the TLUT lookup on the tile's CI format and decoded
    // these texels as literal IA8 -- wrong colors, wrong alpha.
    let mut storage = Tmem::default();
    let tile = Tile {
        fmt: G_IM_FMT_IA,
        siz: G_IM_SIZ_8B,
        line: 9,
        ..Default::default()
    };
    storage.write_texel(tile, 0, 0, false, G_IM_SIZ_8B, 0x42);
    storage.write_tlut(256, 0x42, 0xf801);
    let texture = TmemTexture {
        storage: std::rc::Rc::new(storage),
        tile,
        texture_lut: 2,
    };
    assert_eq!(texture.sample(0, 0), [255, 0, 0, 255]);
}


#[test]
fn load_tile_uses_settimg_stride_and_tile_coordinate_origin() {
    // A synthetic 4x2 CI8 source. Load the rightmost two texels of row 1
    // as a 2x1 tile whose render coordinates begin at (2, 1).
    let base = 0x100usize;
    let mut rdram = vec![0u8; base + 12];
    for (i, index) in (0u8..8).enumerate() {
        wr_u8(&mut rdram, base + i, index);
    }
    let mut tlut = vec![[0, 0, 0, 255]; 8];
    tlut[6] = [60, 61, 62, 255];
    tlut[7] = [70, 71, 72, 255];
    let mut tex = TexState {
        timg_addr: base as u32,
        timg_width: 4,
        tlut,
        ..Default::default()
    };
    tex.tiles[0] = Tile {
        fmt: G_IM_FMT_CI,
        siz: G_IM_SIZ_8B,
        uls: 2 * 4,
        ult: 4,
        lrs: 3 * 4,
        lrt: 4,
        clamp_s: true,
        clamp_t: true,
        ..Default::default()
    };

    let decoded = decode_current_texture(
        &rdram,
        &tex,
        &[0; 16],
        0,
        TextureLoad::Tile {
            source_x: 2,
            source_y: 1,
        },
    );

    assert_eq!(
        decoded.texels.as_slice(),
        &[60, 61, 62, 255, 70, 71, 72, 255]
    );
    assert_eq!(decoded.sample(2.0, 1.0), [60, 61, 62, 255]);
    assert_eq!(decoded.sample(3.0, 1.0), [70, 71, 72, 255]);
}


#[test]
fn load_tile_preserves_equal_fractional_bounds_as_subtexel_origin() {
    let base = 0x100usize;
    let mut rdram = vec![0u8; base + 8];
    for (index, value) in [10, 20, 30, 40].into_iter().enumerate() {
        wr_u8(&mut rdram, base + index, value);
    }
    let mut tex = TexState {
        timg_addr: base as u32,
        timg_fmt: G_IM_FMT_I,
        timg_siz: G_IM_SIZ_8B,
        timg_width: 4,
        ..Default::default()
    };
    tex.tiles[0] = Tile {
        fmt: G_IM_FMT_I,
        siz: G_IM_SIZ_8B,
        line: 1,
        clamp_s: true,
        clamp_t: true,
        ..Default::default()
    };

    // Load source texels 1..=2 with a quarter-texel S origin and a
    // half-texel T origin. Table 7 retains the fractions in tile state;
    // equal low/high fractions select the same integer source span.
    load_tile_into_tmem(
        &rdram,
        &mut tex,
        &[0; 16],
        0,
        (u32::from(G_LOADTILE) << 24) | (5 << 12) | 2,
        (9 << 12) | 2,
    );
    let texture = bind_texture_set(&tex, 0, 0, 0).expect("fractional tile must bind");
    assert_eq!(texture.origin_s, 1.25);
    assert_eq!(texture.origin_t, 0.5);
    assert_eq!(texture.sample(1.25, 0.5), [20, 20, 20, 20]);
    assert_eq!(texture.sample(2.25, 0.5), [30, 30, 30, 30]);
}


#[test]
fn load_tile_unequal_fractional_edges_select_integer_span_and_retain_bounds() {
    let mut tex = TexState {
        timg_addr: 0,
        timg_fmt: G_IM_FMT_I,
        timg_siz: G_IM_SIZ_8B,
        timg_width: 1,
        ..Default::default()
    };
    tex.tiles[0] = Tile {
        fmt: G_IM_FMT_I,
        siz: G_IM_SIZ_8B,
        line: 1,
        ..Default::default()
    };
    load_tile_into_tmem(
        &[0x7f; 8],
        &mut tex,
        &[0; 16],
        0,
        u32::from(G_LOADTILE) << 24,
        2 << 12,
    );
    assert_eq!((tex.tiles[0].uls, tex.tiles[0].lrs), (0, 2));
    let texture = bind_texture_set(&tex, 0, 0, 0).expect("fractional tile must bind");
    assert_eq!(texture.sample(0.0, 0.0), [0x7f; 4]);
}


#[test]
fn load_tile_uses_texture_image_size_for_rgba32_split_storage() {
    let base = 0x100usize;
    let mut rdram = vec![0u8; base + 12];
    for (index, value) in [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80]
        .into_iter()
        .enumerate()
    {
        wr_u8(&mut rdram, base + index, value);
    }
    let mut tex = TexState {
        timg_addr: base as u32,
        timg_fmt: G_IM_FMT_RGBA,
        timg_siz: G_IM_SIZ_32B,
        timg_width: 2,
        ..Default::default()
    };
    // The public Set Tile usage note requires a 16-bit load descriptor
    // for RGBA32 even though the source-sized transfer is split across
    // low/high TMEM halves.
    tex.tiles[7] = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_16B,
        line: 1,
        ..Default::default()
    };
    load_tile_into_tmem(
        &rdram,
        &mut tex,
        &[0; 16],
        7,
        u32::from(G_LOADTILE) << 24,
        (7 << 24) | (4 << 12),
    );
    tex.tiles[0] = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_32B,
        line: 1,
        lrs: 4,
        ..Default::default()
    };
    let texture = bind_texture_set(&tex, 0, 0, 0).expect("RGBA32 tile must bind");
    assert_eq!(texture.sample(0.0, 0.0), [0x10, 0x20, 0x30, 0x40]);
    assert_eq!(texture.sample(1.0, 0.0), [0x50, 0x60, 0x70, 0x80]);
}


#[test]
fn load_block_counts_source_sized_texels_with_mismatched_load_tile() {
    let base = 0x100usize;
    let mut rdram = vec![0u8; base + 12];
    for (index, value) in [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
        .into_iter()
        .enumerate()
    {
        wr_u8(&mut rdram, base + index, value);
    }
    let mut tex = TexState {
        timg_addr: base as u32,
        timg_fmt: G_IM_FMT_RGBA,
        timg_siz: G_IM_SIZ_32B,
        timg_width: 2,
        ..Default::default()
    };
    tex.tiles[7] = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_16B,
        ..Default::default()
    };
    load_block_into_tmem(
        &rdram,
        &mut tex,
        &[0; 16],
        7,
        u32::from(G_LOADBLOCK) << 24,
        (7 << 24) | (1 << 12),
    );
    tex.tiles[0] = Tile {
        fmt: G_IM_FMT_RGBA,
        siz: G_IM_SIZ_32B,
        line: 1,
        lrs: 4,
        ..Default::default()
    };
    let texture = bind_texture_set(&tex, 0, 0, 0).expect("RGBA32 block must bind");
    assert_eq!(texture.sample(0.0, 0.0), [0x11, 0x22, 0x33, 0x44]);
    assert_eq!(texture.sample(1.0, 0.0), [0x55, 0x66, 0x77, 0x88]);
}


#[test]
fn decode_rgba16_covers_low_channels_and_alpha_edges() {
    // 0x0001 = opaque black; 0xffff = opaque white; 0x0842 has the
    // lowest nonzero R/G/B codes and clear alpha. This catches both a
    // dropped 1-bit alpha and incorrect 5-to-8 scaling at the low edge.
    assert_texture_row(
        &[0x00, 0x01, 0xff, 0xff, 0x08, 0x42],
        3,
        G_IM_FMT_RGBA,
        G_IM_SIZ_16B,
        0,
        Vec::new(),
        &[0, 0, 0, 255, 255, 255, 255, 255, 8, 8, 8, 0],
    );
}


#[test]
fn decode_rgba8_uses_observed_hardware_i8_alias() {
    // Fail-against-bug: this pair previously fell through to None and
    // left the surface flat. RT64 records that hardware samples it as I8.
    assert_texture_row(
        &[0x24, 0xdb],
        2,
        G_IM_FMT_RGBA,
        G_IM_SIZ_8B,
        0,
        Vec::new(),
        &[0x24, 0x24, 0x24, 0x24, 0xdb, 0xdb, 0xdb, 0xdb],
    );
}


#[test]
fn decode_rgba4_uses_observed_hardware_i4_alias() {
    // Fail-against-bug and live-OoT case: RGBA4 was one of the `_ =>
    // None` combinations, so every such tile remained flat-shaded.
    assert_texture_row(
        &[0x39],
        2,
        G_IM_FMT_RGBA,
        G_IM_SIZ_4B,
        0,
        Vec::new(),
        &[0x33, 0x33, 0x33, 0x33, 0x99, 0x99, 0x99, 0x99],
    );
}


#[test]
fn decode_ia8_splits_four_bit_intensity_and_alpha() {
    assert_texture_row(
        &[0x1e, 0xf0],
        2,
        G_IM_FMT_IA,
        G_IM_SIZ_8B,
        0,
        Vec::new(),
        &[0x11, 0x11, 0x11, 0xee, 0xff, 0xff, 0xff, 0x00],
    );
}


#[test]
fn decode_ia4_is_three_bit_intensity_plus_one_bit_alpha() {
    // Fail-against-bug: the old shared I4/IA4 arm expanded the whole
    // nibble into every channel. In particular 0x1 became translucent
    // dark gray and 0xe became opaque light gray. IA4 requires those to
    // be opaque black and transparent white respectively.
    assert_texture_row(
        &[0x1e, 0xa7],
        4,
        G_IM_FMT_IA,
        G_IM_SIZ_4B,
        0,
        Vec::new(),
        &[
            0, 0, 0, 255, // 0x1: I=0, A=1
            255, 255, 255, 0, // 0xe: I=7, A=0
            182, 182, 182, 0, // 0xa: I=5, A=0
            109, 109, 109, 255, // 0x7: I=3, A=1
        ],
    );
}


#[test]
fn decode_i8_replicates_intensity_into_rgba() {
    assert_texture_row(
        &[0x00, 0x7f, 0xff],
        3,
        G_IM_FMT_I,
        G_IM_SIZ_8B,
        0,
        Vec::new(),
        &[0, 0, 0, 0, 0x7f, 0x7f, 0x7f, 0x7f, 0xff, 0xff, 0xff, 0xff],
    );
}
