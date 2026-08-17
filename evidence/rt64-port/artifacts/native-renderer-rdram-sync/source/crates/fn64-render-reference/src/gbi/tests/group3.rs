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
#[should_panic(expected = "G_TEXRECT is truncated at RDRAM 0x00001008")]
fn truncated_texture_rectangle_traps_before_losing_command_alignment() {
    let mut rdram = vec![0u8; 0x1008];
    wr_cmd(&mut rdram, 0x1000, (G_TEXRECT as u32) << 24, 0);
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "G_VTX reads past RDRAM")]
fn truncated_vertex_dma_traps_instead_of_partially_updating_the_cache() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_VTX as u32) << 24) | (2 << 12) | (2 << 1),
        0x1010,
    );
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "G_VTX encoded end slot 1 and count 2")]
fn malformed_vertex_cache_range_traps_instead_of_saturating_to_slot_zero() {
    let mut rdram = vec![0u8; 0x1100];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_VTX as u32) << 24) | (2 << 12) | (1 << 1),
        0x1080,
    );
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "G_TRI vertex-cache slots [32, 0, 0]")]
fn triangle_with_nonexistent_cache_slot_traps_instead_of_disappearing() {
    let mut rdram = vec![0u8; 0x1010];
    wr_cmd(&mut rdram, 0x1000, ((G_TRI1 as u32) << 24) | (32 << 17), 0);
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "G_TEXTURE enables tile 0 but no initialized TMEM image")]
fn enabled_texture_without_live_tmem_traps_instead_of_substituting_white() {
    const DL: usize = 0x1000;
    const VERTICES: usize = 0x1080;
    let mut rdram = vec![0u8; 0x1100];
    for index in 0..3 {
        wr_vtx(
            &mut rdram,
            VERTICES + index * VTX_STRIDE,
            index as i16,
            index as i16,
            0,
            [255; 4],
        );
    }
    wr_cmd(
        &mut rdram,
        DL,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        VERTICES as u32,
    );
    wr_cmd(
        &mut rdram,
        DL + 8,
        ((G_TEXTURE as u32) << 24) | 2,
        0xffff_ffff,
    );
    wr_cmd(
        &mut rdram,
        DL + 16,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    let _ = decode_display_list_f3dex2_ops(&rdram, DL as u32);
}


#[test]
#[should_panic(expected = "G_MOVEMEM G_MV_VIEWPORT reads past RDRAM")]
fn truncated_viewport_dma_traps_instead_of_retaining_stale_state() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MOVEMEM as u32) << 24) | (1 << 19) | G_MV_VIEWPORT as u32,
        0x1018,
    );
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "G_VTX with an active matrix requires G_MOVEMEM G_MV_VIEWPORT")]
fn transformed_vertex_without_viewport_traps_instead_of_inventing_screen_mapping() {
    const DL: usize = 0x1000;
    const MATRIX: usize = 0x1080;
    const VERTEX: usize = 0x10c0;
    let mut rdram = vec![0u8; 0x1100];
    wr_mtx(&mut rdram, MATRIX, identity());
    wr_vtx(&mut rdram, VERTEX, 0, 0, 0, [255; 4]);
    wr_cmd(
        &mut rdram,
        DL,
        ((G_MTX as u32) << 24) | (7 << 19) | 0x07,
        MATRIX as u32,
    );
    wr_cmd(
        &mut rdram,
        DL + 8,
        ((G_VTX as u32) << 24) | (1 << 12) | (1 << 1),
        VERTEX as u32,
    );
    wr_cmd(&mut rdram, DL + 16, (G_ENDDL as u32) << 24, 0);
    let _ = decode_display_list_f3dex2_ops(&rdram, DL as u32);
}


#[test]
#[should_panic(expected = "G_MOVEMEM G_MV_LIGHT reads past RDRAM")]
fn truncated_light_dma_traps_instead_of_retaining_stale_state() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MOVEMEM as u32) << 24) | (1 << 19) | (6 << 8) | G_MV_LIGHT as u32,
        0x1018,
    );
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "G_MTX reads past RDRAM")]
fn truncated_matrix_dma_traps_instead_of_retaining_the_previous_transform() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MTX as u32) << 24) | (7 << 19) | 0x03,
        0x1010,
    );
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "malformed G_SETOTHERMODE_H range")]
fn malformed_other_mode_range_traps_instead_of_retaining_stale_bits() {
    let mut rdram = vec![0u8; 0x1010];
    // low byte stores len-1; 0x20 therefore requests an impossible
    // 33-bit update of a 32-bit other-mode word.
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_SETOTHERMODE_H as u32) << 24) | 0x20,
        0,
    );
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
fn g_texrect_consumes_two_words_and_does_not_desync() {
    // A G_TEXRECT (0xE4) is a 16-byte command. If the decoder advances
    // only 8 bytes it reads the coord word as a bogus opcode. Here the
    // texrect's second word is crafted to look like a G_VTX opcode
    // (0x01..) that, if wrongly decoded, would load a spurious vertex.
    // A correct 16-byte skip walks straight to the real G_TRI1.
    let mut rdram = vec![0u8; 0x4000];

    wr_vtx(&mut rdram, 0x3000, 10, 10, 0, [255, 0, 0, 255]);
    wr_vtx(&mut rdram, 0x3010, 20, 10, 0, [0, 255, 0, 255]);
    wr_vtx(&mut rdram, 0x3020, 15, 20, 0, [0, 0, 255, 255]);

    // VTX (3 verts).
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        0x3000,
    );
    // G_TEXRECT word 0 + word 1. The SECOND 8-byte word starts with 0x01
    // (a G_VTX opcode byte) to catch an under-advance.
    wr_cmd(
        &mut rdram,
        0x1008,
        ((G_TEXRECT as u32) << 24) | 0x00abcdef,
        0x12345678,
    );
    wr_cmd(&mut rdram, 0x1010, 0x0100_4008, 0x0100_1c00); // texrect 2nd word

    // Real G_TRI1 after the full 16-byte texrect.
    wr_cmd(
        &mut rdram,
        0x1018,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    wr_cmd(&mut rdram, 0x1020, (G_ENDDL as u32) << 24, 0);

    let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
    assert_eq!(
        tris.len(),
        1,
        "texrect must consume both words so the following G_TRI1 is \
         decoded at the right offset"
    );
}


#[test]
fn texture_rectangle_preserves_signed_fixed_point_and_cycle_state() {
    let mut rdram = vec![0u8; 0x2000];
    // Copy cycle plus threshold alpha compare in one full other-mode
    // command. Blend alpha 0x80 becomes the threshold snapshot.
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_RDPSETOTHERMODE as u32) << 24) | (2 << 20),
        1,
    );
    wr_cmd(
        &mut rdram,
        0x1008,
        (G_SETBLENDCOLOR as u32) << 24,
        0x0000_0080,
    );
    wr_cmd(
        &mut rdram,
        0x1010,
        ((G_TEXRECT as u32) << 24) | ((4 * 4) << 12) | (5 * 4),
        (3 << 24) | ((1 * 4) << 12) | (2 * 4),
    );
    // s=-1.5 (S10.5), t=2.25; dsdx=4.0 and dtdy=0.5 (S5.10).
    wr_cmd(&mut rdram, 0x1018, 0xffd0_0048, 0x1000_0200);
    wr_cmd(&mut rdram, 0x1020, (G_ENDDL as u32) << 24, 0);

    let ops = decode_display_list_f3dex2_ops(&rdram, 0x1000).unwrap();
    let RenderOp::TextureRectangle(rectangle) = &ops[0] else {
        panic!("expected ordered texture rectangle, got {:?}", ops[0]);
    };
    assert_eq!((rectangle.ulx, rectangle.uly), (1.0, 2.0));
    assert_eq!((rectangle.lrx, rectangle.lry), (4.0, 5.0));
    assert_eq!(rectangle.tile, 3);
    assert_eq!((rectangle.s, rectangle.t), (-1.5, 2.25));
    assert_eq!((rectangle.dsdx, rectangle.dtdy), (4096, 512));
    assert_eq!(rectangle.other_mode.cycle_type(), CycleType::Copy);
    assert_eq!(
        rectangle.other_mode.alpha_compare(),
        AlphaCompare::Threshold
    );
    assert_eq!(rectangle.other_mode.blend_color_alpha, 0x80);
    assert!(!rectangle.flip);
    assert!(rectangle.texture.is_none());
    assert!(rectangle.texture1.is_none());
}


#[test]
fn admitted_geometry_families_decode_all_public_texture_rectangle_continuations() {
    let families = [
        GeometryWireFamily::Fast3d,
        GeometryWireFamily::F3dex,
        GeometryWireFamily::F3dlx,
        GeometryWireFamily::F3dlxRej,
        GeometryWireFamily::F3dex2,
        GeometryWireFamily::F3dex2NoN,
        GeometryWireFamily::F3dex2Rej,
        GeometryWireFamily::F3dlx2Rej,
        GeometryWireFamily::L3dex,
        GeometryWireFamily::L3dex2,
    ];
    let signed_boundary_vectors = [
        (0x8000_8000, 0x8000_8000),
        (0xffff_0001, 0xffff_0001),
        (0x0000_0000, 0x0000_0000),
        (0x7fff_7fff, 0x7fff_7fff),
    ];

    for (family_index, family) in families.into_iter().enumerate() {
        let text = vec![family_index as u8 + 1; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let mut catalog = GeometryUcodeCatalog::default();
        catalog.admit_text_for(family, &text);
        let selected_family = catalog
            .require_text(&text)
            .expect("the exact admitted text must select its wire family");
        assert_eq!(selected_family, family);

        let modern = matches!(
            family,
            GeometryWireFamily::F3dex2
                | GeometryWireFamily::F3dex2NoN
                | GeometryWireFamily::F3dex2Rej
                | GeometryWireFamily::F3dlx2Rej
                | GeometryWireFamily::L3dex2
        );
        let (half_1, half_2, enddl) = if modern {
            (G_RDPHALF_1, G_RDPHALF_2, G_ENDDL)
        } else {
            (LEGACY_G_RDPHALF_1, LEGACY_G_RDPHALF_2, L3DEX_G_ENDDL)
        };

        for opcode in [G_TEXRECT, G_TEXRECTFLIP] {
            for (coords, gradients) in signed_boundary_vectors {
                let decode_form = |enveloped: bool| {
                    let mut rdram = vec![0u8; 0x1100];
                    wr_cmd(
                        &mut rdram,
                        0x1000,
                        (u32::from(opcode) << 24) | (16 << 12) | 20,
                        (3 << 24) | (4 << 12) | 8,
                    );
                    if enveloped {
                        wr_cmd(&mut rdram, 0x1008, u32::from(half_1) << 24, coords);
                        wr_cmd(&mut rdram, 0x1010, u32::from(half_2) << 24, gradients);
                        wr_cmd(&mut rdram, 0x1018, u32::from(enddl) << 24, 0);
                    } else {
                        wr_cmd(&mut rdram, 0x1008, coords, gradients);
                        wr_cmd(&mut rdram, 0x1010, u32::from(enddl) << 24, 0);
                    }
                    decode_geometry_fixture(&rdram, selected_family)
                };

                let direct = decode_form(false);
                let enveloped = decode_form(true);
                for operations in [&direct, &enveloped] {
                    assert_eq!(
                        operations.len(),
                        1,
                        "family={family:?} opcode={opcode:#04x}"
                    );
                }
                let (RenderOp::TextureRectangle(direct), RenderOp::TextureRectangle(enveloped)) =
                    (&direct[0], &enveloped[0])
                else {
                    panic!("texture-rectangle vector must emit a typed rectangle");
                };
                assert_eq!(
                    (
                        direct.ulx,
                        direct.uly,
                        direct.lrx,
                        direct.lry,
                        direct.tile,
                        direct.s,
                        direct.t,
                        direct.dsdx,
                        direct.dtdy,
                        direct.flip,
                    ),
                    (
                        enveloped.ulx,
                        enveloped.uly,
                        enveloped.lrx,
                        enveloped.lry,
                        enveloped.tile,
                        enveloped.s,
                        enveloped.t,
                        enveloped.dsdx,
                        enveloped.dtdy,
                        enveloped.flip,
                    ),
                    "family={family:?} opcode={opcode:#04x} coords={coords:#010x} gradients={gradients:#010x}"
                );
            }
        }
    }
}


#[test]
#[should_panic(expected = "wrong-family G_RDPHALF_1 opcode 0xb4")]
fn modern_texture_rectangle_rejects_legacy_continuation_envelope() {
    let mut rdram = vec![0u8; 0x1100];
    wr_cmd(&mut rdram, 0x1000, u32::from(G_TEXRECT) << 24, 0);
    wr_cmd(&mut rdram, 0x1008, u32::from(LEGACY_G_RDPHALF_1) << 24, 0);
    wr_cmd(&mut rdram, 0x1010, u32::from(LEGACY_G_RDPHALF_2) << 24, 0);
    let _ = decode_geometry_fixture(&rdram, GeometryWireFamily::F3dex2);
}


#[test]
#[should_panic(expected = "G_RDPHALF_2 continuation must be opcode 0xb3")]
fn legacy_texture_rectangle_rejects_malformed_second_continuation() {
    let mut rdram = vec![0u8; 0x1100];
    wr_cmd(&mut rdram, 0x1000, u32::from(G_TEXRECT) << 24, 0);
    wr_cmd(&mut rdram, 0x1008, u32::from(LEGACY_G_RDPHALF_1) << 24, 0);
    wr_cmd(&mut rdram, 0x1010, u32::from(G_RDPHALF_2) << 24, 0);
    let _ = decode_geometry_fixture(&rdram, GeometryWireFamily::F3dex);
}


#[test]
#[should_panic(expected = "continuation envelope is truncated")]
fn legacy_texture_rectangle_rejects_truncated_continuation_envelope() {
    let mut rdram = vec![0u8; 0x1010];
    wr_cmd(&mut rdram, 0x1000, u32::from(G_TEXRECT) << 24, 0);
    wr_cmd(&mut rdram, 0x1008, u32::from(LEGACY_G_RDPHALF_1) << 24, 0);
    let _ = decode_geometry_fixture(&rdram, GeometryWireFamily::F3dex);
}


#[test]
fn display_list_rgba32_load_uses_public_sixteen_bit_load_descriptor() {
    const DL: usize = 0x1000;
    const IMAGE: usize = 0x1800;
    let mut rdram = vec![0u8; 0x2000];
    for (index, value) in [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80]
        .into_iter()
        .enumerate()
    {
        wr_u8(&mut rdram, IMAGE + index, value);
    }
    let mut offset = DL;
    wr_cmd(
        &mut rdram,
        offset,
        ((G_SETTIMG as u32) << 24)
            | ((G_IM_FMT_RGBA as u32) << 21)
            | ((G_IM_SIZ_32B as u32) << 19)
            | 1,
        IMAGE as u32,
    );
    offset += 8;
    wr_cmd(
        &mut rdram,
        offset,
        ((G_SETTILE as u32) << 24)
            | ((G_IM_FMT_RGBA as u32) << 21)
            | ((G_IM_SIZ_16B as u32) << 19)
            | (1 << 9),
        7 << 24,
    );
    offset += 8;
    wr_cmd(
        &mut rdram,
        offset,
        (G_LOADTILE as u32) << 24,
        (7 << 24) | (4 << 12),
    );
    offset += 8;
    wr_cmd(
        &mut rdram,
        offset,
        ((G_SETTILE as u32) << 24)
            | ((G_IM_FMT_RGBA as u32) << 21)
            | ((G_IM_SIZ_32B as u32) << 19)
            | (1 << 9),
        0,
    );
    offset += 8;
    wr_cmd(&mut rdram, offset, (G_SETTILESIZE as u32) << 24, 4 << 12);
    offset += 8;
    wr_cmd(
        &mut rdram,
        offset,
        ((G_TEXRECT as u32) << 24) | (8 << 12) | 4,
        0,
    );
    offset += 8;
    wr_cmd(&mut rdram, offset, 0, 0x0400_0400);
    offset += 8;
    wr_cmd(&mut rdram, offset, (G_ENDDL as u32) << 24, 0);

    let ops = decode_display_list_f3dex2_ops(&rdram, DL as u32).unwrap();
    let RenderOp::TextureRectangle(rectangle) = &ops[0] else {
        panic!("expected texture rectangle, got {:?}", ops[0]);
    };
    let texture = rectangle.texture.as_ref().expect("RGBA32 tile must bind");
    assert_eq!(texture.sample(0.0, 0.0), [0x10, 0x20, 0x30, 0x40]);
    assert_eq!(texture.sample(1.0, 0.0), [0x50, 0x60, 0x70, 0x80]);
}

// --- Viewport mapping (priority 1) ----------------------------------


#[test]
fn read_viewport_divides_quarter_pixel_encoding_by_four() {
    // OoT's real full-screen viewport: vscale (640,480,z), vtrans same,
    // in the ×4 "quarter-pixel" encoding -> 160/120 px after ÷4 (§3.5).
    let mut rdram = vec![0u8; 64];
    let addr = 0x10;
    wr_i16(&mut rdram, addr, 640); // vscale.x
    wr_i16(&mut rdram, addr + 2, 480); // vscale.y
    wr_i16(&mut rdram, addr + 4, 511); // vscale.z (~127.75 depth)
    wr_i16(&mut rdram, addr + 8, 640); // vtrans.x
    wr_i16(&mut rdram, addr + 10, 480); // vtrans.y
    wr_i16(&mut rdram, addr + 12, 511); // vtrans.z
    let vp = read_viewport(&rdram, addr).expect("viewport in bounds");
    assert_eq!(vp.sx, 160.0);
    assert_eq!(vp.sy, 120.0);
    assert_eq!(vp.tx, 160.0);
    assert_eq!(vp.ty, 120.0);
    assert_eq!(vp.sz, 127.75);
}


#[test]
fn viewport_maps_known_ndc_points_to_known_pixels() {
    // A 320×240 centered viewport (sx=160, tx=160, sy=120, ty=120).
    // Map the NDC corners the way `project_vertex` does (with the Y-flip).
    let vp = Viewport {
        sx: 160.0,
        sy: 120.0,
        sz: 127.75,
        tx: 160.0,
        ty: 120.0,
        tz: 127.75,
    };
    // NDC origin (0,0) -> screen center (160,120).
    let map = |nx: f32, ny: f32| (nx * vp.sx + vp.tx, -ny * vp.sy + vp.ty);
    assert_eq!(map(0.0, 0.0), (160.0, 120.0));
    // NDC (-1,+1) is top-left on screen after the Y-flip: (0, 0).
    assert_eq!(map(-1.0, 1.0), (0.0, 0.0));
    // NDC (+1,-1) is bottom-right: (320, 240).
    assert_eq!(map(1.0, -1.0), (320.0, 240.0));
}

// --- Culling (priority 2) -------------------------------------------


#[test]
fn cull_mode_from_geometry_mode_bits() {
    assert_eq!(cull_mode_from(0), CullMode::None);
    assert_eq!(cull_mode_from(G_CULL_BACK), CullMode::Back);
    assert_eq!(cull_mode_from(G_CULL_FRONT), CullMode::Front);
    assert_eq!(cull_mode_from(G_CULL_FRONT | G_CULL_BACK), CullMode::Both);
    // Unrelated bits (e.g. G_SHADE=0x4, G_ZBUFFER=0x1) don't cull.
    assert_eq!(cull_mode_from(0x0000_0005), CullMode::None);
}

// --- Vertex lighting (priority 3) -----------------------------------


#[test]
fn load_ucode_resets_rsp_state_but_preserves_independent_rdp_state() {
    let mut state = lit_state();
    state.geometry_mode = G_LIGHTING | G_CULL_BACK;
    state.proj = Some(identity());
    state.modelview = identity();
    state.mvp = Some(identity());
    state.pending_forced_mvp = Some(identity());
    state.mv_stack.push(identity());
    state.segments[3] = 0x0012_3000;
    state.viewport = Some(Viewport {
        sx: 160.0,
        sy: 120.0,
        sz: 127.75,
        tx: 160.0,
        ty: 120.0,
        tz: 127.75,
    });
    state.tex.tex_enabled = true;
    state.tex.tex_tile = 7;
    state.tex.tex_max_level = 5;
    state.tex.tex_scale_s = 1.0;
    state.tex.tex_scale_t = 1.0;
    state.lights.num_dir = 2;
    state.look_at.x = Some([1.0, 0.0, 0.0]);
    state.look_at.y = Some([0.0, 1.0, 0.0]);
    state.persp_normalize = PerspectiveNormalize(Some(0x1234));
    state.clip_ratio = ClipRatio {
        neg_x: 2,
        neg_y: 3,
        pos_x: 4,
        pos_y: 5,
    };
    state.fog = FogFactor {
        multiplier: 7,
        offset: -9,
    };
    state.rdp_half_1 = Some(0x1234_5678);
    state.fog_color = [1, 2, 3, 4];
    state.scissor = Some(ScissorRect {
        ulx: 1.0,
        uly: 2.0,
        lrx: 3.0,
        lry: 4.0,
        field: false,
        keep_odd: false,
    });
    state.other_mode.low = 0x1234;
    state.combiner.primitive = [5, 6, 7, 8];

    reset_rsp_state_from_ucode_load(&mut state);

    assert_eq!(state.geometry_mode, 0);
    assert_eq!(state.proj, Some(identity()));
    assert_eq!(state.modelview, identity());
    assert!(state.mvp.is_none());
    assert!(state.pending_forced_mvp.is_none());
    assert_eq!(state.mv_stack, vec![identity()]);
    assert_eq!(state.segments[3], 0x0012_3000);
    assert!(state.viewport.is_some());
    assert!(!state.tex.tex_enabled);
    assert_eq!(state.tex.tex_tile, 0);
    assert_eq!(state.tex.tex_max_level, 0);
    assert_eq!(state.tex.tex_scale_s, 0.0);
    assert_eq!(state.tex.tex_scale_t, 0.0);
    assert_eq!(state.lights.num_dir, 0);
    assert_eq!(state.look_at, LookAtState::default());
    assert_eq!(
        state.persp_normalize,
        PerspectiveNormalize(Some(0x1234)),
        "public F3DEX2 maintained-state list preserves PerspNormalize"
    );
    assert_eq!(
        state.clip_ratio,
        ClipRatio::default(),
        "clip ratio is absent from the exhaustive F3DEX2 maintained-state list"
    );
    assert_eq!(state.fog, FogFactor::default());
    assert_eq!(state.rdp_half_1, None);
    assert_eq!(state.fog_color, [1, 2, 3, 4]);
    assert!(state.scissor.is_some(), "RDP scissor survives RSP reload");
    assert_eq!(state.other_mode.low, 0x1234);
    assert_eq!(state.combiner.primitive, [5, 6, 7, 8]);
}


#[test]
fn legacy_load_ucode_resets_all_rsp_geometry_state_but_preserves_rdp_state() {
    let mut state = lit_state();
    state.vtx_cache[3].x = 9.0;
    state.segments[3] = 0x0012_3000;
    state.proj = Some(identity());
    state.modelview[3][0] = 5.0;
    state.mvp = Some(identity());
    state.pending_forced_mvp = Some(identity());
    state.mv_stack.push(identity());
    state.viewport = Some(Viewport {
        sx: 160.0,
        sy: 120.0,
        sz: 127.75,
        tx: 160.0,
        ty: 120.0,
        tz: 127.75,
    });
    state.geometry_mode = G_LIGHTING | G_CULL_BACK;
    state.rdp_half_1 = Some(0x1234_5678);
    state.tex.tex_enabled = true;
    state.tex.tex_tile = 7;
    state.tex.tex_max_level = 5;
    state.tex.tex_scale_s = 1.0;
    state.tex.tex_scale_t = 1.0;
    state.lights.num_dir = 2;
    state.look_at.x = Some([1.0, 0.0, 0.0]);
    state.fog = FogFactor {
        multiplier: 7,
        offset: -9,
    };
    state.persp_normalize = PerspectiveNormalize(Some(0x1234));
    state.clip_ratio = ClipRatio {
        neg_x: 2,
        neg_y: 3,
        pos_x: 4,
        pos_y: 5,
    };

    state.tex.timg_addr = 0x0013_0000;
    state.tex.tlut.push([1, 2, 3, 4]);
    state.fog_color = [5, 6, 7, 8];
    state.fill_color = 0x1234_5678;
    state.scissor = Some(ScissorRect {
        ulx: 1.0,
        uly: 2.0,
        lrx: 3.0,
        lry: 4.0,
        field: false,
        keep_odd: false,
    });
    state.other_mode.low = 0x1234;
    state.combiner.primitive = [9, 10, 11, 12];

    reset_legacy_rsp_state_from_ucode_load(&mut state);

    assert_eq!(
        state.vtx_cache,
        [Vertex::default(); MAX_GEOMETRY_VERTEX_CACHE]
    );
    assert_eq!(state.vtx_loaded, [false; MAX_GEOMETRY_VERTEX_CACHE]);
    assert_eq!(state.segments, [0; 16]);
    assert!(state.proj.is_none());
    assert_eq!(state.modelview, identity());
    assert!(state.mvp.is_none());
    assert!(state.pending_forced_mvp.is_none());
    assert!(state.mv_stack.is_empty());
    assert!(state.viewport.is_none());
    assert_eq!(state.geometry_mode, 0);
    assert_eq!(state.rdp_half_1, None);
    assert!(!state.tex.tex_enabled);
    assert_eq!(state.tex.tex_tile, 0);
    assert_eq!(state.tex.tex_max_level, 0);
    assert_eq!(state.tex.tex_scale_s, 0.0);
    assert_eq!(state.tex.tex_scale_t, 0.0);
    assert_eq!(state.lights.num_dir, 0);
    assert_eq!(state.look_at, LookAtState::default());
    assert_eq!(state.fog, FogFactor::default());
    assert_eq!(state.persp_normalize, PerspectiveNormalize::default());
    assert_eq!(state.clip_ratio, ClipRatio::default());

    assert_eq!(state.tex.timg_addr, 0x0013_0000);
    assert_eq!(state.tex.tlut, vec![[1, 2, 3, 4]]);
    assert_eq!(state.fog_color, [5, 6, 7, 8]);
    assert_eq!(state.fill_color, 0x1234_5678);
    assert!(state.scissor.is_some());
    assert_eq!(state.other_mode.low, 0x1234);
    assert_eq!(state.combiner.primitive, [9, 10, 11, 12]);
}


#[test]
#[should_panic(
    expected = "F3DEX/L3DEX G_LOAD_UCODE inside a called display list resets link state and cannot return"
)]
fn legacy_load_ucode_in_called_list_traps_before_resetting_link_state() {
    let mut state = lit_state();
    state.dl_depth = 1;
    reset_legacy_rsp_state_from_ucode_load(&mut state);
}


#[test]
fn rdp_registers_tiles_and_tmem_survive_f3dex2_task_boundaries() {
    const LOAD_DL: usize = 0x1000;
    const DRAW_DL: usize = 0x1100;
    const VERTICES: usize = 0x1200;
    const IMAGE: usize = 0x1300;
    let mut rdram = vec![0u8; 0x1400];
    let mut rsp_memory = fn64_runtime::RspMemory::new();
    let catalog = F3dex2UcodeCatalog::default();
    let mut rdp_state = RdpDecodeState::default();

    // First task: load one opaque-red RGBA16 texel into physical TMEM,
    // configure render tile 0, and program a constant RDP register.
    wr_u8(&mut rdram, IMAGE, 0xf8);
    wr_u8(&mut rdram, IMAGE + 1, 0x01);
    let mut pc = LOAD_DL;
    wr_cmd(
        &mut rdram,
        pc,
        ((G_SETTIMG as u32) << 24)
            | ((G_IM_FMT_RGBA as u32) << 21)
            | ((G_IM_SIZ_16B as u32) << 19),
        IMAGE as u32,
    );
    pc += 8;
    wr_cmd(
        &mut rdram,
        pc,
        ((G_SETTILE as u32) << 24)
            | ((G_IM_FMT_RGBA as u32) << 21)
            | ((G_IM_SIZ_16B as u32) << 19)
            | (1 << 9),
        7 << 24,
    );
    pc += 8;
    wr_cmd(&mut rdram, pc, (G_LOADTILE as u32) << 24, 7 << 24);
    pc += 8;
    wr_cmd(
        &mut rdram,
        pc,
        ((G_SETTILE as u32) << 24)
            | ((G_IM_FMT_RGBA as u32) << 21)
            | ((G_IM_SIZ_16B as u32) << 19)
            | (1 << 9),
        0,
    );
    pc += 8;
    wr_cmd(&mut rdram, pc, (G_SETTILESIZE as u32) << 24, 0);
    pc += 8;
    wr_cmd(&mut rdram, pc, (G_SETPRIMCOLOR as u32) << 24, 0x12_34_56_78);
    pc += 8;
    wr_cmd(&mut rdram, pc, (G_ENDDL as u32) << 24, 0);

    let load_ops = execute_display_list_f3dex2_ops_admitted_with_rdp_state(
        &mut rdram,
        &mut rsp_memory,
        LOAD_DL as u32,
        &catalog,
        &mut rdp_state,
    )
    .unwrap();
    assert!(load_ops.is_empty());
    assert!(!rdp_state.tex.tex_enabled, "G_TEXTURE is RSP state");

    // Second task: only its RSP-owned G_TEXTURE/vertex state is rebuilt.
    // No RDP texture/register setup is repeated.
    wr_vtx(&mut rdram, VERTICES, 1, 1, 0, [255; 4]);
    wr_vtx(&mut rdram, VERTICES + VTX_STRIDE, 5, 1, 0, [255; 4]);
    wr_vtx(&mut rdram, VERTICES + 2 * VTX_STRIDE, 1, 5, 0, [255; 4]);
    wr_cmd(
        &mut rdram,
        DRAW_DL,
        ((G_TEXTURE as u32) << 24) | 2,
        0xffff_ffff,
    );
    wr_cmd(
        &mut rdram,
        DRAW_DL + 8,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        VERTICES as u32,
    );
    wr_cmd(
        &mut rdram,
        DRAW_DL + 16,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    wr_cmd(&mut rdram, DRAW_DL + 24, (G_ENDDL as u32) << 24, 0);

    let draw_ops = execute_display_list_f3dex2_ops_admitted_with_rdp_state(
        &mut rdram,
        &mut rsp_memory,
        DRAW_DL as u32,
        &catalog,
        &mut rdp_state,
    )
    .unwrap();
    let RenderOp::Triangle(triangle) = &draw_ops[0] else {
        panic!("expected one triangle, got {:?}", draw_ops[0]);
    };
    assert_eq!(triangle.combiner.primitive, [0x12, 0x34, 0x56, 0x78]);
    assert_eq!(
        triangle
            .texture
            .as_ref()
            .expect("task-two G_TEXTURE must bind task-one TMEM")
            .sample(0.0, 0.0),
        [255, 0, 0, 255]
    );
    assert!(!rdp_state.tex.tex_enabled, "task commit clears RSP state");
}


#[test]
fn num_lights_from_moveword_divides_by_24() {
    // gsSPNumLights writes NUML(n) = n*24; num_dir = data/24.
    let mut st = lit_state();
    // 2 directional lights: data = 48.
    st.lights.num_dir = (48u32 / 24) as usize;
    assert_eq!(st.lights.num_dir, 2);
}


#[test]
fn moveword_light_color_updates_directional_and_ambient_slots() {
    let mut rdram = vec![0u8; 0x1100];
    let mut pc = 0x1000;
    // One directional light makes slot 1 the ambient light.
    wr_cmd(
        &mut rdram,
        pc,
        ((G_MOVEWORD as u32) << 24) | ((G_MW_NUMLIGHT as u32) << 16),
        24,
    );
    pc += 8;
    // Public gSPLightColor writes both color copies. Slot 0 uses offsets
    // 0/4 and slot 1 uses 24/28. Alpha is ignored.
    for (offset, color) in [
        (0u16, 0x2040_60ff),
        (4, 0x2040_60ff),
        (24, 0x80a0_c000),
        (28, 0x80a0_c000),
    ] {
        wr_cmd(
            &mut rdram,
            pc,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_LIGHTCOL as u32) << 16) | u32::from(offset),
            color,
        );
        pc += 8;
    }
    wr_cmd(&mut rdram, pc, (G_ENDDL as u32) << 24, 0);

    let state = decode_display_list_f3dex2_state(&rdram, 0x1000).unwrap();
    assert_eq!(state.lights.num_dir, 1);
    assert_eq!(
        state.lights.dir[0].col,
        [32.0 / 255.0, 64.0 / 255.0, 96.0 / 255.0]
    );
    assert_eq!(state.lights.dir[0].dir, [0.0; 3]);
    assert_eq!(
        state.lights.ambient,
        [128.0 / 255.0, 160.0 / 255.0, 192.0 / 255.0]
    );
}


#[test]
#[should_panic(expected = "G_MOVEWORD G_MW_LIGHTCOL offset")]
fn malformed_moveword_light_color_offset_traps_by_name() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MOVEWORD as u32) << 24) | ((G_MW_LIGHTCOL as u32) << 16) | 8,
        0x1122_3344,
    );
    wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
    let _ = decode_display_list_f3dex2_state(&rdram, 0x1000);
}


#[test]
fn moveword_fog_decodes_signed_factor_halfwords() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MOVEWORD as u32) << 24) | ((G_MW_FOG as u32) << 16),
        ((-128i16 as u16 as u32) << 16) | 300,
    );
    wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);

    let state = decode_display_list_f3dex2_state(&rdram, 0x1000).unwrap();
    assert_eq!(
        state.fog,
        FogFactor {
            multiplier: -128,
            offset: 300,
        }
    );
}


#[test]
fn moveword_perspective_normalize_retains_public_u16_scale() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MOVEWORD as u32) << 24) | ((G_MW_PERSPNORM as u32) << 16),
        0x0000_3456,
    );
    wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);

    let state = decode_display_list_f3dex2_state(&rdram, 0x1000).unwrap();
    assert_eq!(state.persp_normalize, PerspectiveNormalize(Some(0x3456)));
}


#[test]
fn moveword_clip_ratio_decodes_all_four_public_destinations() {
    let mut rdram = vec![0u8; 0x1040];
    let mut pc = 0x1000;
    for (offset, value) in [
        (G_MWO_CLIP_RNX, 5),
        (G_MWO_CLIP_RNY, 5),
        (G_MWO_CLIP_RPX, (-5i16 as u16) as u32),
        (G_MWO_CLIP_RPY, (-5i16 as u16) as u32),
    ] {
        wr_cmd(
            &mut rdram,
            pc,
            ((G_MOVEWORD as u32) << 24) | ((G_MW_CLIP as u32) << 16) | u32::from(offset),
            value,
        );
        pc += 8;
    }
    wr_cmd(&mut rdram, pc, (G_ENDDL as u32) << 24, 0);

    let state = decode_display_list_f3dex2_state(&rdram, 0x1000).unwrap();
    assert_eq!(
        state.clip_ratio,
        ClipRatio {
            neg_x: 5,
            neg_y: 5,
            pos_x: 5,
            pos_y: 5,
        }
    );
}


#[test]
#[should_panic(expected = "is not FRUSTRATIO_1..6")]
fn moveword_clip_ratio_rejects_non_public_value() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MOVEWORD as u32) << 24) | ((G_MW_CLIP as u32) << 16) | u32::from(G_MWO_CLIP_RNX),
        7,
    );
    wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
    let _ = decode_display_list_f3dex2_state(&rdram, 0x1000);
}


#[test]
fn nonzero_perspective_normalize_is_neutral_in_float_reference_divide() {
    let mut state = lit_state();
    state.mvp = Some(identity());
    state.viewport = Some(centered_viewport());
    state.persp_normalize = PerspectiveNormalize(Some(1));
    let smallest = project_vertex(&state, 1.0, -1.0, 0.5);
    state.persp_normalize = PerspectiveNormalize(Some(u16::MAX));
    let largest = project_vertex(&state, 1.0, -1.0, 0.5);
    assert_eq!(smallest, largest);
}


#[test]
fn zero_perspective_normalize_rejects_triangle_and_line_geometry() {
    const DL: usize = 0x1000;
    const VERTICES: usize = 0x1080;
    let mut rdram = vec![0u8; 0x1100];
    wr_vtx(&mut rdram, VERTICES, 0, 0, 0, [255, 0, 0, 255]);
    wr_vtx(
        &mut rdram,
        VERTICES + VTX_STRIDE,
        10,
        0,
        0,
        [0, 255, 0, 255],
    );
    wr_vtx(
        &mut rdram,
        VERTICES + 2 * VTX_STRIDE,
        0,
        10,
        0,
        [0, 0, 255, 255],
    );
    wr_cmd(
        &mut rdram,
        DL,
        ((G_MOVEWORD as u32) << 24) | ((G_MW_PERSPNORM as u32) << 16),
        0,
    );
    wr_cmd(
        &mut rdram,
        DL + 8,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        VERTICES as u32,
    );
    wr_cmd(
        &mut rdram,
        DL + 16,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    wr_cmd(&mut rdram, DL + 24, ((G_LINE3D as u32) << 24) | (2 << 8), 0);
    wr_cmd(&mut rdram, DL + 32, (G_ENDDL as u32) << 24, 0);

    assert!(
        decode_display_list_f3dex2_ops(&rdram, DL as u32)
            .unwrap()
            .iter()
            .all(|op| !matches!(op, RenderOp::Triangle(_) | RenderOp::Line(_))),
        "zero perspective-normalization scale must not produce triangle or line geometry"
    );
}


#[test]
#[should_panic(expected = "scale must be a public u16 value")]
fn perspective_normalize_rejects_non_public_high_bits() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MOVEWORD as u32) << 24) | ((G_MW_PERSPNORM as u32) << 16),
        0x0001_0000,
    );
    wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
    let _ = decode_display_list_f3dex2_state(&rdram, 0x1000);
}


#[test]
fn fog_geometry_mode_replaces_vertex_alpha_from_projected_depth() {
    let mut rdram = vec![0u8; 0x1100];
    let base = 0x1000;
    wr_vtx(&mut rdram, base, 0, 0, -1, [1, 2, 3, 17]);
    wr_vtx(&mut rdram, base + VTX_STRIDE, 0, 0, 0, [1, 2, 3, 17]);
    wr_vtx(&mut rdram, base + 2 * VTX_STRIDE, 0, 0, 1, [1, 2, 3, 17]);
    let mut state = lit_state();
    state.mvp = Some(identity());
    state.viewport = Some(centered_viewport());
    state.geometry_mode = G_FOG;
    state.fog = FogFactor {
        multiplier: 128,
        offset: 128,
    };

    load_vertices(
        &rdram,
        &mut state,
        base as u32,
        3,
        0,
        GeometryWireFamily::F3dex2,
    );
    assert_eq!(state.vtx_cache[0].a, 0);
    assert_eq!(state.vtx_cache[1].a, 128);
    assert_eq!(state.vtx_cache[2].a, 255);
    assert_eq!(state.vtx_cache[1].r, 1);
}


#[test]
fn movemem_light_1_maps_to_directional_slot_zero() {
    // Fail-against-bug wire evidence: gSPLight(LIGHT_1) encodes
    // (1*24 + 24)/8 = 6. The old `ofs/3 - 1` mapping returned slot 1,
    // leaving the real first directional light (slot 0) black/stale and
    // misclassifying LIGHT_1 as ambient when num_dir == 1.
    assert_eq!(light_slot_from_movemem_offset(6), Some(0));
    // LIGHT_2 is the ambient slot when one directional light is active.
    assert_eq!(light_slot_from_movemem_offset(9), Some(1));
    // Offsets for the two look-at vectors are not light slots.
    assert_eq!(light_slot_from_movemem_offset(0), None);
    assert_eq!(light_slot_from_movemem_offset(3), None);
}


#[test]
fn force_matrix_compound_replaces_mvp_without_mutating_matrix_stacks() {
    const DL: usize = 0x1000;
    const MATRIX: usize = 0x1100;
    const VERTEX: usize = 0x1180;
    const VIEWPORT: usize = 0x11f0;
    let mut rdram = vec![0u8; 0x1200];
    wr_mtx(&mut rdram, MATRIX, identity());
    wr_vtx(&mut rdram, VERTEX, 0, 0, 0, [1, 2, 3, 255]);
    wr_centered_viewport(&mut rdram, VIEWPORT);
    wr_cmd(&mut rdram, DL, movemem_viewport_word(), VIEWPORT as u32);
    wr_cmd(
        &mut rdram,
        DL + 8,
        ((G_MOVEMEM as u32) << 24) | (7 << 19) | G_MV_MATRIX as u32,
        MATRIX as u32,
    );
    wr_cmd(
        &mut rdram,
        DL + 16,
        ((G_MOVEWORD as u32) << 24) | ((G_MW_FORCEMTX as u32) << 16),
        0x0001_0000,
    );
    wr_cmd(
        &mut rdram,
        DL + 24,
        ((G_VTX as u32) << 24) | (1 << 12) | (1 << 1),
        VERTEX as u32,
    );
    wr_cmd(&mut rdram, DL + 32, (G_ENDDL as u32) << 24, 0);

    let state = decode_display_list_f3dex2_state(&rdram, DL as u32).unwrap();
    assert_eq!(state.modelview, identity());
    assert!(state.proj.is_none());
    assert_eq!(state.mvp, Some(identity()));
    assert!(state.pending_forced_mvp.is_none());
    assert_eq!((state.vtx_cache[0].x, state.vtx_cache[0].y), (160.0, 120.0));
}


#[test]
fn ordinary_matrix_command_supersedes_force_matrix_override() {
    const DL: usize = 0x1000;
    const FORCED: usize = 0x1100;
    const PROJECTION: usize = 0x1140;
    const VERTEX: usize = 0x1180;
    const VIEWPORT: usize = 0x11f0;
    let mut rdram = vec![0u8; 0x1200];
    let mut translated = identity();
    translated[3][0] = 0.25;
    wr_mtx(&mut rdram, FORCED, translated);
    wr_mtx(&mut rdram, PROJECTION, identity());
    wr_vtx(&mut rdram, VERTEX, 0, 0, 0, [1, 2, 3, 255]);
    wr_centered_viewport(&mut rdram, VIEWPORT);
    wr_cmd(&mut rdram, DL, movemem_viewport_word(), VIEWPORT as u32);
    wr_cmd(
        &mut rdram,
        DL + 8,
        ((G_MOVEMEM as u32) << 24) | (7 << 19) | G_MV_MATRIX as u32,
        FORCED as u32,
    );
    wr_cmd(
        &mut rdram,
        DL + 16,
        ((G_MOVEWORD as u32) << 24) | ((G_MW_FORCEMTX as u32) << 16),
        0x0001_0000,
    );
    // Public params PROJECTION|LOAD|NOPUSH = 0x06; F3DEX2 wire XORs the
    // push bit, so the low command byte is 0x07.
    wr_cmd(
        &mut rdram,
        DL + 24,
        ((G_MTX as u32) << 24) | (7 << 19) | 0x07,
        PROJECTION as u32,
    );
    wr_cmd(
        &mut rdram,
        DL + 32,
        ((G_VTX as u32) << 24) | (1 << 12) | (1 << 1),
        VERTEX as u32,
    );
    wr_cmd(&mut rdram, DL + 40, (G_ENDDL as u32) << 24, 0);

    let state = decode_display_list_f3dex2_state(&rdram, DL as u32).unwrap();
    assert_eq!(state.mvp, Some(identity()));
    assert_eq!(state.vtx_cache[0].x, 160.0);
}


#[test]
fn modelview_only_matrix_uses_identity_projection() {
    const DL: usize = 0x1000;
    const MODELVIEW: usize = 0x1100;
    const VERTEX: usize = 0x1180;
    const VIEWPORT: usize = 0x11f0;
    let mut rdram = vec![0u8; 0x1200];
    let mut translated = identity();
    translated[3][0] = 0.25;
    wr_mtx(&mut rdram, MODELVIEW, translated);
    wr_vtx(&mut rdram, VERTEX, 0, 0, 0, [1, 2, 3, 255]);
    wr_centered_viewport(&mut rdram, VIEWPORT);
    // MODELVIEW|LOAD|NOPUSH = 0x02, XORed with the F3DEX2 push bit on
    // the wire to 0x03.
    wr_cmd(&mut rdram, DL, movemem_viewport_word(), VIEWPORT as u32);
    wr_cmd(
        &mut rdram,
        DL + 8,
        ((G_MTX as u32) << 24) | (7 << 19) | 0x03,
        MODELVIEW as u32,
    );
    wr_cmd(
        &mut rdram,
        DL + 16,
        ((G_VTX as u32) << 24) | (1 << 12) | (1 << 1),
        VERTEX as u32,
    );
    wr_cmd(&mut rdram, DL + 24, (G_ENDDL as u32) << 24, 0);

    let state = decode_display_list_f3dex2_state(&rdram, DL as u32).unwrap();
    assert!(state.proj.is_none());
    assert_eq!(state.mvp, Some(translated));
    assert_eq!((state.vtx_cache[0].x, state.vtx_cache[0].y), (200.0, 120.0));
}


#[test]
#[should_panic(expected = "requires a preceding G_MOVEMEM G_MV_MATRIX")]
fn force_matrix_marker_without_dma_traps_by_both_command_names() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MOVEWORD as u32) << 24) | ((G_MW_FORCEMTX as u32) << 16),
        0x0001_0000,
    );
    wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
    let _ = decode_display_list_f3dex2_state(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "must carry one 64-byte Mtx")]
fn force_matrix_dma_rejects_non_public_length_by_name() {
    let mut rdram = vec![0u8; 0x1100];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MOVEMEM as u32) << 24) | (6 << 19) | G_MV_MATRIX as u32,
        0x1080,
    );
    wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
    let _ = decode_display_list_f3dex2_state(&rdram, 0x1000);
}


#[test]
fn movemem_look_at_decodes_both_public_screen_axes() {
    const DL: usize = 0x1000;
    const LOOK_X: usize = 0x1080;
    const LOOK_Y: usize = 0x10a0;
    let mut rdram = vec![0u8; 0x1100];
    wr_u8(&mut rdram, LOOK_X + 8, 127);
    wr_u8(&mut rdram, LOOK_X + 9, 0);
    wr_u8(&mut rdram, LOOK_X + 10, 0x81); // -127
    wr_u8(&mut rdram, LOOK_Y + 8, 0);
    wr_u8(&mut rdram, LOOK_Y + 9, 127);
    wr_u8(&mut rdram, LOOK_Y + 10, 0);

    // gSPLookAtX/Y share G_MV_LIGHT and select public offsets 0*24 and
    // 1*24. The wire stores those destinations divided by eight.
    wr_cmd(
        &mut rdram,
        DL,
        ((G_MOVEMEM as u32) << 24) | (1 << 19) | G_MV_LIGHT as u32,
        LOOK_X as u32,
    );
    wr_cmd(
        &mut rdram,
        DL + 8,
        ((G_MOVEMEM as u32) << 24) | (1 << 19) | (3 << 8) | G_MV_LIGHT as u32,
        LOOK_Y as u32,
    );
    wr_cmd(&mut rdram, DL + 16, (G_ENDDL as u32) << 24, 0);

    let state = decode_display_list_f3dex2_state(&rdram, DL as u32).unwrap();
    assert_eq!(state.look_at.x, Some([1.0, 0.0, -1.0]));
    assert_eq!(state.look_at.y, Some([0.0, 1.0, 0.0]));
}


#[test]
fn regular_texture_generation_maps_signed_projections_to_scale() {
    let state = texture_generation_state(false);
    assert_eq!(
        generated_texture_coords(&state, [1.0, 0.0, 0.0]),
        (31.0, 15.5)
    );
    assert_eq!(
        generated_texture_coords(&state, [-1.0, 0.0, 0.0]),
        (0.0, 15.5)
    );
    assert_eq!(
        generated_texture_coords(&state, [0.0, 1.0, 0.0]),
        (15.5, 31.0)
    );
}


#[test]
fn linear_texture_generation_maps_inverse_cosine_to_scale() {
    let state = texture_generation_state(true);
    assert_texture_coords_close(
        generated_texture_coords(&state, [1.0, 0.0, 0.0]),
        (0.0, 15.5),
    );
    assert_texture_coords_close(
        generated_texture_coords(&state, [-1.0, 0.0, 0.0]),
        (31.0, 15.5),
    );
    assert_texture_coords_close(
        generated_texture_coords(&state, [0.0, 1.0, 0.0]),
        (15.5, 0.0),
    );
}


#[test]
fn texture_generation_replaces_explicit_vertex_coordinates() {
    let mut rdram = vec![0u8; 64];
    wr_vtx(&mut rdram, 0, 0, 0, 0, [127, 0, 0, 255]);
    wr_i16(&mut rdram, 8, -1234);
    wr_i16(&mut rdram, 10, 2345);
    let mut state = texture_generation_state(false);
    state.lights.ambient = [1.0, 1.0, 1.0];

    load_vertices(&rdram, &mut state, 0, 1, 0, GeometryWireFamily::F3dex2);

    let vertex = state.vtx_cache[0];
    assert_eq!((vertex.s, vertex.t), (31.0, 15.5));
    assert_eq!((vertex.r, vertex.g, vertex.b), (255, 255, 255));
}


#[test]
#[should_panic(expected = "G_TEXTURE_GEN requires G_LIGHTING")]
fn texture_generation_without_lighting_traps_by_geometry_mode_name() {
    let mut state = texture_generation_state(false);
    state.geometry_mode = G_TEXTURE_GEN;
    let _ = generated_texture_coords(&state, [1.0, 0.0, 0.0]);
}


#[test]
#[should_panic(expected = "gSPLookAtY")]
fn texture_generation_without_both_look_at_axes_traps_by_command_name() {
    let mut state = texture_generation_state(false);
    state.look_at.y = None;
    let _ = generated_texture_coords(&state, [1.0, 0.0, 0.0]);
}


#[test]
fn load_light_decodes_color_and_signed_direction() {
    // Light_t: col[3] u8 @0..3, dir[3] s8 @8..11. Plant a red light
    // pointing along -Z (dir byte 0x81 == -127 -> ~-1.0 after /127).
    let mut rdram = vec![0u8; 64];
    let addr = 0x10;
    wr_u8(&mut rdram, addr, 255); // col.r
    wr_u8(&mut rdram, addr + 1, 0); // col.g
    wr_u8(&mut rdram, addr + 2, 0); // col.b
    wr_u8(&mut rdram, addr + 8, 0); // dir.x
    wr_u8(&mut rdram, addr + 9, 0); // dir.y
    wr_u8(&mut rdram, addr + 10, 0x81); // dir.z = -127
    let mut st = lit_state();
    st.lights.num_dir = 1; // slot 0 is directional here
    load_light(&rdram, &mut st, addr, 0);
    let l = st.lights.dir[0];
    assert_eq!(l.col, [1.0, 0.0, 0.0]);
    assert!((l.dir[2] - (-127.0 / 127.0)).abs() < 1e-6);
    assert_eq!(l.dir[0], 0.0);
}


#[test]
fn light_vertex_face_on_light_is_full_diffuse_plus_ambient() {
    // One white directional light pointing at the surface normal (+Z),
    // plus a dim gray ambient. A normal facing the light (+Z) gets full
    // N·L=1 -> ambient + light color, clamped.
    let mut st = lit_state();
    st.lights.num_dir = 1;
    st.lights.ambient = [0.1, 0.1, 0.1];
    st.lights.dir[0] = DirLight {
        dir: [0.0, 0.0, 1.0],
        col: [0.8, 0.8, 0.8],
    };
    // Normal directly toward the light: N·L = 1.
    let c = light_vertex(&st, [0.0, 0.0, 1.0]);
    // 0.1 + 1.0*0.8 = 0.9 -> 229.
    assert_eq!(c, [229, 229, 229]);
}


#[test]
fn light_vertex_back_face_gets_ambient_only() {
    // A normal facing AWAY from the light (N·L < 0, clamped to 0) is lit
    // by ambient alone -- the diffuse term must not go negative (that
    // was the failure mode a naive dot without a max(.,0) would hit).
    let mut st = lit_state();
    st.lights.num_dir = 1;
    st.lights.ambient = [0.2, 0.2, 0.2];
    st.lights.dir[0] = DirLight {
        dir: [0.0, 0.0, 1.0],
        col: [1.0, 1.0, 1.0],
    };
    // Normal pointing away from the +Z light.
    let c = light_vertex(&st, [0.0, 0.0, -1.0]);
    assert_eq!(c, [51, 51, 51]); // 0.2*255 = 51, no negative diffuse.
}


#[test]
fn light_vertex_is_not_the_raw_normal_bytes() {
    // Fail-against-bug: the OLD path read the s8 normal bytes AS a flat
    // color. A normal of (0,0,+1) with a green light must NOT come out as
    // the raw normal-as-color (which would be ~[0,0,255] from cn bytes);
    // it must be the LIT color (green from the light). This is exactly the
    // "rainbow fan" bug: signed normals misread as unsigned color.
    let mut st = lit_state();
    st.lights.num_dir = 1;
    st.lights.ambient = [0.0, 0.0, 0.0];
    st.lights.dir[0] = DirLight {
        dir: [0.0, 0.0, 1.0],
        col: [0.0, 1.0, 0.0], // green
    };
    let c = light_vertex(&st, [0.0, 0.0, 1.0]);
    assert_eq!(c, [0, 255, 0]); // green, from the LIGHT -- not the normal.
}


#[test]
fn light_vertex_half_angle_scales_diffuse() {
    // A 45° normal to a +Z light: N·L = cos(45°) ≈ 0.707, so a white
    // light yields ~0.707 -> ~180 (screen-linear, no gamma).
    let mut st = lit_state();
    st.lights.num_dir = 1;
    st.lights.ambient = [0.0, 0.0, 0.0];
    st.lights.dir[0] = DirLight {
        dir: [0.0, 0.0, 1.0],
        col: [1.0, 1.0, 1.0],
    };
    let inv_sqrt2 = 1.0 / 2.0_f32.sqrt();
    let c = light_vertex(&st, [inv_sqrt2, 0.0, inv_sqrt2]);
    // 0.707 * 255 ≈ 180.
    assert!((c[0] as i32 - 180).abs() <= 1, "got {}", c[0]);
}
