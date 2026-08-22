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
fn digest_selected_family_resolves_colliding_triangle_opcode() {
    let mut rdram = vec![0u8; 0x2100];
    for (slot, (x, y)) in [(1, 1), (5, 1), (1, 5)].into_iter().enumerate() {
        wr_vtx(&mut rdram, 0x2000 + slot * VTX_STRIDE, x, y, 0, [255; 4]);
    }
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        0x2000,
    );
    // This exact word is F3DEX2 G_TRI1 slots 0/1/2, but the public line
    // microcode contract interprets G_TRI1 as a validated NOOP.
    wr_cmd(
        &mut rdram,
        0x1008,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);

    let polygon = decode_geometry_fixture(&rdram, GeometryWireFamily::F3dex2);
    let line = decode_geometry_fixture(&rdram, GeometryWireFamily::L3dex2);
    assert_eq!(polygon.len(), 1);
    assert!(matches!(polygon[0], RenderOp::Triangle(_)));
    assert!(line.is_empty());
}


#[test]
fn admitted_polygon_families_public_triangle_forms_rasterize_identically() {
    let families = [
        GeometryWireFamily::Fast3d,
        GeometryWireFamily::F3dex,
        GeometryWireFamily::F3dlx,
        GeometryWireFamily::F3dlxRej,
        GeometryWireFamily::F3dex2,
        GeometryWireFamily::F3dex2NoN,
        GeometryWireFamily::F3dex2Rej,
        GeometryWireFamily::F3dlx2Rej,
    ];
    let pixels = families.map(|family| {
        let operations = equivalent_polygon_fixture(family);
        assert_eq!(operations.len(), 1);
        let RenderOp::Triangle(triangle) = &operations[0] else {
            panic!("polygon fixture must emit one typed triangle");
        };
        let mut framebuffer = crate::raster::Framebuffer::new(8, 8);
        framebuffer.draw_triangle(triangle);
        assert!(framebuffer.pixels.iter().any(|component| *component != 0));
        framebuffer.pixels
    });
    for pair in pixels.windows(2) {
        assert_eq!(pair[0], pair[1]);
    }
}


#[test]
fn f3dlx_rej_uses_public_sixty_four_slot_cache_with_thirty_two_vertex_load_limit() {
    let mut rdram = vec![0u8; 0x2100];
    for (slot, (x, y)) in [(1, 1), (6, 1), (1, 6)].into_iter().enumerate() {
        wr_vtx(
            &mut rdram,
            0x2000 + slot * VTX_STRIDE,
            x,
            y,
            0,
            [255, 255, 255, 255],
        );
    }
    wr_cmd(
        &mut rdram,
        0x1000,
        ((L3DEX_G_VTX as u32) << 24) | (64 << 16) | (3 << 10) | 47,
        0x2000,
    );
    wr_cmd(
        &mut rdram,
        0x1008,
        (L3DEX_G_TRI1 as u32) << 24,
        (64 << 16) | (66 << 8) | 68,
    );
    wr_cmd(&mut rdram, 0x1010, (L3DEX_G_ENDDL as u32) << 24, 0);

    let rejected_by_f3dex =
        std::panic::catch_unwind(|| decode_geometry_fixture(&rdram, GeometryWireFamily::F3dex));
    assert!(rejected_by_f3dex.is_err());
    let operations = decode_geometry_fixture(&rdram, GeometryWireFamily::F3dlxRej);
    assert_eq!(operations.len(), 1);
    let RenderOp::Triangle(triangle) = &operations[0] else {
        panic!("F3DLX.Rej high-cache fixture must emit one triangle");
    };
    assert_eq!(triangle.v[0].x, 1.0);
    assert_eq!(triangle.v[1].x, 6.0);
    assert_eq!(triangle.v[2].y, 6.0);
}


#[test]
fn f3dex2_rej_variants_load_all_sixty_four_vertices_in_one_command() {
    let mut rdram = vec![0u8; 0x2500];
    for (slot, (x, y)) in [(1, 1), (6, 1), (1, 6)].into_iter().enumerate() {
        wr_vtx(
            &mut rdram,
            0x2000 + (61 + slot) * VTX_STRIDE,
            x,
            y,
            0,
            [255, 255, 255, 255],
        );
    }
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_VTX as u32) << 24) | (64 << 12) | (64 << 1),
        0x2000,
    );
    wr_cmd(
        &mut rdram,
        0x1008,
        ((G_TRI1 as u32) << 24) | (61 << 17) | (62 << 9) | (63 << 1),
        0,
    );
    wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);

    let rejected_by_standard = std::panic::catch_unwind(|| {
        decode_geometry_fixture(&rdram, GeometryWireFamily::F3dex2)
    });
    assert!(rejected_by_standard.is_err());
    for family in [GeometryWireFamily::F3dex2Rej, GeometryWireFamily::F3dlx2Rej] {
        let operations = decode_geometry_fixture(&rdram, family);
        assert_eq!(operations.len(), 1);
        let RenderOp::Triangle(triangle) = &operations[0] else {
            panic!("modern Rej high-cache fixture must emit one triangle");
        };
        assert_eq!(triangle.v[0].x, 1.0);
        assert_eq!(triangle.v[1].x, 6.0);
        assert_eq!(triangle.v[2].y, 6.0);
    }
}


#[test]
fn f3dzex2_variants_and_public_non_disable_the_reference_near_admission_gate() {
    let mut cache = [Vertex::default(); 64];
    for vertex in &mut cache[..3] {
        vertex.w = 0.0;
        vertex.clip_position = Some([0.0, 0.0, -1.0, 0.0]);
    }
    let resolve = |profile| {
        resolve_tri_for_profile(
            &cache,
            [0, 1, 2],
            profile,
            0,
            ClipRatio::default(),
            CullMode::None,
            None,
            OtherMode::default(),
            CombinerState::default(),
            BlenderState::default(),
        )
    };
    assert!(resolve(GeometryUcodeProfile::from_public_family(
        GeometryWireFamily::F3dex2
    ))
    .is_none());
    assert!(resolve(GeometryUcodeProfile::from_public_family(
        GeometryWireFamily::F3dex2NoN
    ))
    .is_some());
    for variant in [
        F3dzex2Variant::NoNFifo206H,
        F3dzex2Variant::NoNFifo208I,
        F3dzex2Variant::NoNFifo208J,
    ] {
        assert!(
            resolve(f3dzex2_profile(variant)).is_some(),
            "{variant:?} did not apply its typed NoN profile"
        );
    }
}


#[test]
fn f3dzex2_non_changes_only_near_admission_in_the_bounded_triangle_model() {
    let ordinary = GeometryUcodeProfile::from_public_family(GeometryWireFamily::F3dex2);
    let profiles = [
        f3dzex2_profile(F3dzex2Variant::NoNFifo206H),
        f3dzex2_profile(F3dzex2Variant::NoNFifo208I),
        f3dzex2_profile(F3dzex2Variant::NoNFifo208J),
    ];
    for (clip_position, clip_code) in [
        ([2.0, 0.0, 0.0, 1.0], CLIP_POS_X),
        ([-2.0, 0.0, 0.0, 1.0], CLIP_NEG_X),
        ([0.0, 2.0, 0.0, 1.0], CLIP_POS_Y),
        ([0.0, -2.0, 0.0, 1.0], CLIP_NEG_Y),
        ([0.0, 0.0, 2.0, 1.0], CLIP_POS_Z),
    ] {
        let mut cache = [Vertex::default(); 64];
        for vertex in &mut cache[..3] {
            vertex.w = 1.0;
            vertex.clip_position = Some(clip_position);
            vertex.clip_code = clip_code;
        }
        let resolve = |profile| {
            resolve_tri_for_profile(
                &cache,
                [0, 1, 2],
                profile,
                0,
                ClipRatio::default(),
                CullMode::None,
                None,
                OtherMode::default(),
                CombinerState::default(),
                BlenderState::default(),
            )
            .expect("positive-W side/far fixture remains a raster clipping handoff")
        };
        let ordinary_triangle = resolve(ordinary);
        assert!(ordinary_triangle
            .v
            .iter()
            .all(|vertex| vertex.clip_code == clip_code));
        for profile in profiles {
            assert_eq!(resolve(profile).v, ordinary_triangle.v);
        }
    }
}


#[test]
fn typed_f3dzex2_profiles_keep_other_special_opcodes_reserved() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(&mut rdram, 0x1000, (G_SPECIAL_1 as u32) << 24, 0);
    wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
    for variant in [
        F3dzex2Variant::NoNFifo206H,
        F3dzex2Variant::NoNFifo208I,
        F3dzex2Variant::NoNFifo208J,
    ] {
        let result = std::panic::catch_unwind(|| {
            decode_geometry_fixture_profile(&rdram, f3dzex2_profile(variant))
        });
        assert!(result.is_err(), "{variant:?} accepted reserved G_SPECIAL_1");
    }
}


#[test]
fn public_family_boundary_still_maps_f3dex2_non_without_typed_f3dzex2() {
    let mut cache = [Vertex::default(); 64];
    for vertex in &mut cache[..3] {
        vertex.w = 0.0;
    }
    assert!(resolve_tri_for_family(
        &cache,
        [0, 1, 2],
        GeometryWireFamily::F3dex2,
        0,
        ClipRatio::default(),
        CullMode::None,
        None,
        OtherMode::default(),
        CombinerState::default(),
        BlenderState::default(),
    )
    .is_none());
    assert!(resolve_tri_for_family(
        &cache,
        [0, 1, 2],
        GeometryWireFamily::F3dex2NoN,
        0,
        ClipRatio::default(),
        CullMode::None,
        None,
        OtherMode::default(),
        CombinerState::default(),
        BlenderState::default(),
    )
    .is_some());
}


#[test]
#[should_panic(
    expected = "F3DLX2.Rej transformed G_VTX requires exact pixel-precision rounding that the public manuals do not specify"
)]
fn f3dlx2_rej_transformed_vertex_precision_remains_loud() {
    let mut state = lit_state();
    state.mvp = Some(identity());
    load_vertices(
        &[0; VTX_STRIDE],
        &mut state,
        0,
        1,
        0,
        GeometryWireFamily::F3dlx2Rej,
    );
}


#[test]
fn public_f3dex2_variants_keep_special_opcodes_reserved() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(&mut rdram, 0x1000, (G_SPECIAL_1 as u32) << 24, 0);
    wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
    for family in [
        GeometryWireFamily::F3dex2,
        GeometryWireFamily::F3dex2NoN,
        GeometryWireFamily::F3dex2Rej,
        GeometryWireFamily::F3dlx2Rej,
    ] {
        let result = std::panic::catch_unwind(|| decode_geometry_fixture(&rdram, family));
        assert!(
            result.is_err(),
            "{} accepted reserved G_SPECIAL_1",
            family.name()
        );
    }
}


#[test]
#[should_panic(
    expected = "F3DZEX2 HLE admission requires a complete allowed-source specification for its family-specific continuation and branch commands"
)]
fn f3dzex2_digest_admission_is_loud_until_its_wire_is_allowed() {
    let mut catalog = GeometryUcodeCatalog::default();
    catalog.admit_text_for(
        GeometryWireFamily::F3dzex2,
        &[0x7a; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    );
}


#[test]
fn f3dlx_clipping_and_rej_cull_modes_are_family_specific() {
    let f3dlx = normalize_legacy_geometry_mode(
        GeometryWireFamily::F3dlx,
        LEGACY_G_CLIPPING | LEGACY_G_CULL_BACK,
    );
    assert_ne!(f3dlx & LEGACY_G_CLIPPING, 0);
    assert_ne!(f3dlx & G_CULL_BACK, 0);

    let f3dex_clipping = std::panic::catch_unwind(|| {
        normalize_legacy_geometry_mode(GeometryWireFamily::F3dex, LEGACY_G_CLIPPING)
    });
    assert!(f3dex_clipping.is_err());
    let rej_front_cull = std::panic::catch_unwind(|| {
        normalize_legacy_geometry_mode(GeometryWireFamily::F3dlxRej, LEGACY_G_CULL_FRONT)
    });
    assert!(rej_front_cull.is_err());
}


#[test]
fn polygon_family_initial_state_matches_public_clip_and_reject_defaults() {
    let mut f3dlx = fresh_decode_state();
    initialize_geometry_family_state(&mut f3dlx, GeometryWireFamily::F3dlx);
    assert_ne!(f3dlx.geometry_mode & LEGACY_G_CLIPPING, 0);

    for family in [
        GeometryWireFamily::F3dlxRej,
        GeometryWireFamily::F3dex2,
        GeometryWireFamily::F3dex2NoN,
        GeometryWireFamily::F3dex2Rej,
        GeometryWireFamily::F3dlx2Rej,
    ] {
        let mut state = fresh_decode_state();
        initialize_geometry_family_state(&mut state, family);
        assert_eq!(
            state.clip_ratio,
            ClipRatio {
                neg_x: 2,
                neg_y: 2,
                pos_x: 2,
                pos_y: 2,
            },
            "{} must begin at public FRUSTRATIO_2",
            family.name()
        );
    }
}


#[test]
fn f3dlx_rej_box_rejects_xy_and_far_but_not_near_vertices() {
    let ratio = ClipRatio {
        neg_x: 2,
        neg_y: 2,
        pos_x: 2,
        pos_y: 2,
    };
    let mut cache = [Vertex::default(); 64];
    for (slot, clip) in [
        [0.0, 0.0, -2.0, 1.0],
        [1.5, 0.0, 0.0, 1.0],
        [0.0, 1.5, 1.0, 1.0],
    ]
    .into_iter()
    .enumerate()
    {
        cache[slot].w = clip[3];
        cache[slot].clip_position = Some(clip);
    }
    assert!(resolve_tri_with_admission(
        &cache,
        [0, 1, 2],
        64,
        TriangleAdmission::RejectBox(ratio),
        CullMode::None,
        None,
        OtherMode::default(),
        CombinerState::default(),
        BlenderState::default(),
    )
    .is_some());

    cache[1].clip_position = Some([2.1, 0.0, 0.0, 1.0]);
    assert!(resolve_tri_with_admission(
        &cache,
        [0, 1, 2],
        64,
        TriangleAdmission::RejectBox(ratio),
        CullMode::None,
        None,
        OtherMode::default(),
        CombinerState::default(),
        BlenderState::default(),
    )
    .is_none());
    cache[1].clip_position = Some([0.0, 0.0, 1.1, 1.0]);
    assert!(resolve_tri_with_admission(
        &cache,
        [0, 1, 2],
        64,
        TriangleAdmission::RejectBox(ratio),
        CullMode::None,
        None,
        OtherMode::default(),
        CombinerState::default(),
        BlenderState::default(),
    )
    .is_none());
}


#[test]
#[should_panic(
    expected = "F3DLX transformed G_VTX requires exact pixel-precision rounding that the public manuals do not specify"
)]
fn f3dlx_transformed_vertex_precision_remains_a_loud_frontier() {
    let mut state = lit_state();
    state.mvp = Some(identity());
    load_vertices(
        &[0; VTX_STRIDE],
        &mut state,
        0,
        1,
        0,
        GeometryWireFamily::F3dlx,
    );
}


#[test]
#[should_panic(expected = "unsupported Fast3D command byte 0xb1")]
fn unpublished_historical_fast3d_quadrangle_form_remains_loud() {
    normalize_geometry_command(
        GeometryWireFamily::Fast3d,
        (F3DEX_G_TRI2 as u32) << 24,
        0,
        0x1000,
    );
}


#[test]
fn published_f3dex_quadrangle_emulation_matches_f3dex2_quad_raster() {
    let legacy = published_quadrangle_fixture(GeometryWireFamily::F3dex);
    let modern = published_quadrangle_fixture(GeometryWireFamily::F3dex2);
    assert_eq!(legacy.len(), 2);
    assert_eq!(modern.len(), 2);

    let raster = |operations: &[RenderOp]| {
        let mut framebuffer = crate::raster::Framebuffer::new(8, 8);
        for operation in operations {
            let RenderOp::Triangle(triangle) = operation else {
                panic!("quadrangle fixture must lower to two triangles");
            };
            framebuffer.draw_triangle(triangle);
        }
        framebuffer.pixels
    };
    assert_eq!(raster(&legacy), raster(&modern));
}


#[test]
fn digest_family_disambiguates_opcode_01_matrix_from_vertex() {
    // The same complete first word is a public 64-byte projection matrix
    // DMA under base/F3DEX envelopes and a 16-vertex load ending at slot
    // 32 under F3DEX2. Only admitted digest identity can choose safely.
    let colliding_w0 = 0x0101_0040;
    let (fast_w0, _) =
        normalize_geometry_command(GeometryWireFamily::Fast3d, colliding_w0, 0x2000, 0x1000);
    let (f3dex_w0, _) =
        normalize_geometry_command(GeometryWireFamily::F3dex, colliding_w0, 0x2000, 0x1000);
    let (modern_w0, _) =
        normalize_geometry_command(GeometryWireFamily::F3dex2, colliding_w0, 0x2000, 0x1000);
    assert_eq!(fast_w0 >> 24, u32::from(G_MTX));
    assert_eq!(f3dex_w0 >> 24, u32::from(G_MTX));
    assert_eq!(modern_w0 >> 24, u32::from(G_VTX));
}


#[test]
fn geometry_catalog_reports_only_admitted_families_and_rejects_collisions() {
    let mut catalog = GeometryUcodeCatalog::default();
    catalog.admit_text_for(
        GeometryWireFamily::Fast3d,
        &[0x01; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    );
    catalog.admit_text_for(
        GeometryWireFamily::F3dex,
        &[0x02; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    );
    catalog.admit_text_for(
        GeometryWireFamily::F3dlx,
        &[0x03; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    );
    catalog.admit_text_for(
        GeometryWireFamily::F3dlxRej,
        &[0x04; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    );
    catalog.admit_text_for(
        GeometryWireFamily::F3dex2,
        &[0x05; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    );
    catalog.admit_text_for(
        GeometryWireFamily::F3dex2NoN,
        &[0x06; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    );
    catalog.admit_text_for(
        GeometryWireFamily::F3dex2Rej,
        &[0x07; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    );
    catalog.admit_text_for(
        GeometryWireFamily::F3dlx2Rej,
        &[0x08; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    );
    catalog.admit_text_for(
        GeometryWireFamily::L3dex,
        &[0x11; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    );
    assert_eq!(
        catalog.supported_ucodes(),
        &[
            UcodeId::Fast3d,
            UcodeId::F3dex,
            UcodeId::F3dlx,
            UcodeId::F3dlxRej,
            UcodeId::F3dex2,
            UcodeId::F3dex2NoN,
            UcodeId::F3dex2Rej,
            UcodeId::F3dlx2Rej,
            UcodeId::L3dex
        ]
    );
    catalog.admit_text_for(
        GeometryWireFamily::L3dex2,
        &[0x22; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    );
    assert_eq!(
        catalog.supported_ucodes(),
        &[
            UcodeId::Fast3d,
            UcodeId::F3dex,
            UcodeId::F3dlx,
            UcodeId::F3dlxRej,
            UcodeId::F3dex2,
            UcodeId::F3dex2NoN,
            UcodeId::F3dex2Rej,
            UcodeId::F3dlx2Rej,
            UcodeId::L3dex,
            UcodeId::L3dex2
        ]
    );

    let digest = [0x5a; 32];
    catalog.admit_sha256_for(GeometryWireFamily::L3dex, digest);
    let collision = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        catalog.admit_sha256_for(GeometryWireFamily::L3dex2, digest);
    }));
    assert!(collision.is_err());
}


#[test]
#[should_panic(expected = "G_LINE3D cache indices must use F3DEX2 v*2 encoding")]
fn malformed_line3d_endpoint_encoding_traps_by_name() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(&mut rdram, 0x1000, ((G_LINE3D as u32) << 24) | (1 << 16), 0);
    wr_cmd(&mut rdram, 0x1008, (G_ENDDL as u32) << 24, 0);
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "without a preceding G_RDPHALF_1 target")]
fn taken_branch_z_without_staged_target_traps_by_name() {
    let mut rdram = vec![0u8; 0x1100];
    wr_cmd(&mut rdram, 0x1000, (G_BRANCH_Z as u32) << 24, u32::MAX);
    decode_display_list_f3dex2_ops(&rdram, 0x1000).unwrap();
}


#[test]
fn pop_matrix_n_restores_the_requested_modelview_depth() {
    const DL: usize = 0x1000;
    const A: usize = 0x2000;
    const B: usize = 0x2100;
    const C: usize = 0x2200;
    let mut rdram = vec![0u8; 0x3000];
    let mut a = identity();
    a[3][0] = 1.0;
    let mut b = identity();
    b[3][0] = 2.0;
    let mut c = identity();
    c[3][0] = 3.0;
    wr_mtx(&mut rdram, A, a);
    wr_mtx(&mut rdram, B, b);
    wr_mtx(&mut rdram, C, c);
    let mtx_len = ((64u32 - 1) / 8) << 19;
    let command = |wire: u32| ((G_MTX as u32) << 24) | mtx_len | wire;
    wr_cmd(&mut rdram, DL, command(0x03), A as u32); // LOAD
    wr_cmd(&mut rdram, DL + 8, command(0x02), B as u32); // LOAD|PUSH
    wr_cmd(&mut rdram, DL + 16, command(0x02), C as u32); // LOAD|PUSH
    wr_cmd(
        &mut rdram,
        DL + 24,
        ((G_POPMTX as u32) << 24) | mtx_len | 2,
        2 * 64,
    );
    wr_cmd(&mut rdram, DL + 32, (G_ENDDL as u32) << 24, 0);

    let state = decode_display_list_f3dex2_state(&rdram, DL as u32).unwrap();
    assert_eq!(state.modelview, a);
    assert!(state.mv_stack.is_empty());
}


#[test]
fn set_convert_decodes_all_signed_nine_bit_fields_in_raw_and_f3dex_streams() {
    let expected = [-256, -43, -1, 222, 114, 255];
    let (w0, w1) = set_convert_command(expected);
    assert_eq!(ConvertState::decode(w0, w1).coefficients, expected);

    let mut rdram = vec![0u8; 0x1100];
    wr_cmd(&mut rdram, 0x1000, w0, w1);
    wr_cmd(
        &mut rdram,
        0x1008,
        ((G_TEXRECT as u32) << 24) | (4 << 12) | 4,
        0,
    );
    wr_cmd(&mut rdram, 0x1010, 0, 0x0400_0400);
    wr_cmd(&mut rdram, 0x1018, (G_ENDDL as u32) << 24, 0);

    for ops in [
        decode_display_list_f3dex2_ops(&rdram, 0x1000).unwrap(),
        decode_raw_rdp_ops(&rdram, 0x1000).unwrap(),
    ] {
        let RenderOp::TextureRectangle(rectangle) = &ops[0] else {
            panic!("expected texture rectangle after G_SETCONVERT");
        };
        assert_eq!(rectangle.combiner.convert.coefficients, expected);
    }

    let mut command_only = vec![0u8; 8];
    wr_cmd(&mut command_only, 0, w0, w1);
    validate_raw_rdp_command_range(&command_only, 0, 8).unwrap();
}


#[test]
fn set_key_commands_converge_across_raw_and_f3dex_streams() {
    let expected = KeyState {
        center: [10, 20, 30],
        scale: [40, 50, 60],
        width: [0x101, 0x100, 0x080],
    };
    let set_gb = (
        ((G_SETKEYGB as u32) << 24)
            | (u32::from(expected.width[1]) << 12)
            | u32::from(expected.width[2]),
        (u32::from(expected.center[1]) << 24)
            | (u32::from(expected.scale[1]) << 16)
            | (u32::from(expected.center[2]) << 8)
            | u32::from(expected.scale[2]),
    );
    let set_r = (
        (G_SETKEYR as u32) << 24,
        (u32::from(expected.width[0]) << 16)
            | (u32::from(expected.center[0]) << 8)
            | u32::from(expected.scale[0]),
    );

    let mut rdram = vec![0u8; 0x1100];
    wr_cmd(&mut rdram, 0x1000, set_gb.0, set_gb.1);
    wr_cmd(&mut rdram, 0x1008, set_r.0, set_r.1);
    wr_cmd(
        &mut rdram,
        0x1010,
        ((G_TEXRECT as u32) << 24) | (4 << 12) | 4,
        0,
    );
    wr_cmd(&mut rdram, 0x1018, 0, 0x0400_0400);
    wr_cmd(&mut rdram, 0x1020, (G_ENDDL as u32) << 24, 0);

    for ops in [
        decode_display_list_f3dex2_ops(&rdram, 0x1000).unwrap(),
        decode_raw_rdp_ops(&rdram, 0x1000).unwrap(),
    ] {
        let RenderOp::TextureRectangle(rectangle) = &ops[0] else {
            panic!("expected texture rectangle after G_SETKEYGB/G_SETKEYR");
        };
        assert_eq!(rectangle.combiner.key, expected);
    }

    validate_raw_rdp_command_range(&rdram, 0x1000, 0x1010).unwrap();
    assert_eq!(expected.alpha_from_key_prime([10.0, 0.0, 0.0]), 0.5);
}


#[test]
fn modify_vertex_updates_all_public_post_transform_cache_fields() {
    let mut rdram = vec![0u8; 0x4000];
    wr_vtx(&mut rdram, 0x3000, 1, 2, 3, [4, 5, 6, 7]);
    wr_vtx(&mut rdram, 0x3010, 20, 2, 3, [8, 9, 10, 11]);
    wr_vtx(&mut rdram, 0x3020, 1, 20, 3, [12, 13, 14, 15]);
    let mut offset = 0x1000;
    wr_cmd(
        &mut rdram,
        offset,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        0x3000,
    );
    offset += 8;
    for (where_field, value) in [
        (G_MWO_POINT_RGBA, 0xA1_B2_C3_D4),
        (G_MWO_POINT_ST, 0x0140_FF60), // s=10.0, t=-5.0 in S10.5
        (G_MWO_POINT_XYSCREEN, 0x0032_FFF3), // x=12.5, y=-3.25 in S13.2
        (G_MWO_POINT_ZSCREEN, 0x01FF_8000), // z=511.5 in 16.16
    ] {
        wr_cmd(
            &mut rdram,
            offset,
            ((G_MODIFYVTX as u32) << 24) | ((where_field as u32) << 16),
            value,
        );
        offset += 8;
    }
    wr_cmd(
        &mut rdram,
        offset,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    offset += 8;
    wr_cmd(&mut rdram, offset, (G_ENDDL as u32) << 24, 0);

    let triangles = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
    assert_eq!(triangles.len(), 1);
    let vertex = triangles[0].v[0];
    assert_eq!(
        [vertex.r, vertex.g, vertex.b, vertex.a],
        [0xA1, 0xB2, 0xC3, 0xD4]
    );
    assert_eq!((vertex.s, vertex.t), (10.0, -5.0));
    assert_eq!((vertex.x, vertex.y), (12.5, -3.25));
    assert_eq!(vertex.z, 511.5);
}


#[test]
#[should_panic(expected = "G_MODIFYVTX cache slot 0 uses unsupported where field 0x20")]
fn modify_vertex_unknown_destination_traps_by_name() {
    let mut rdram = vec![0u8; 0x2000];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MODIFYVTX as u32) << 24) | (0x20 << 16),
        0,
    );
    let _ = decode_display_list_f3dex2(&rdram, 0x1000);
}


#[test]
fn ordered_ops_preserve_color_target_fill_and_full_sync() {
    let mut rdram = vec![0u8; 0x3000];
    let mut offset = 0x1000;
    // Full other-mode write with G_CYC_FILL in high bits 20..21.
    wr_cmd(
        &mut rdram,
        offset,
        ((G_RDPSETOTHERMODE as u32) << 24) | (3 << 20),
        0,
    );
    offset += 8;
    // RGBA16, width=4, target=0x2000.
    wr_cmd(
        &mut rdram,
        offset,
        ((G_SETCIMG as u32) << 24) | (2 << 19) | 3,
        0x2000,
    );
    offset += 8;
    wr_cmd(
        &mut rdram,
        offset,
        (G_SETFILLCOLOR as u32) << 24,
        0xf801_003f,
    );
    offset += 8;
    // Inclusive rectangle (0,0)..(2,1), packed as quarter pixels.
    wr_cmd(
        &mut rdram,
        offset,
        ((G_FILLRECT as u32) << 24) | ((2 * 4) << 12) | (1 * 4),
        0,
    );
    offset += 8;
    wr_cmd(&mut rdram, offset, (G_RDPFULLSYNC as u32) << 24, 0);
    offset += 8;
    wr_cmd(&mut rdram, offset, (G_ENDDL as u32) << 24, 0);

    let ops = decode_display_list_f3dex2_ops(&rdram, 0x1000).unwrap();
    assert_eq!(ops.len(), 3);
    assert!(matches!(
        ops[0],
        RenderOp::SetColorImage(ColorImage {
            format: 0,
            size: 2,
            width: 4,
            address: 0x2000,
        })
    ));
    assert!(matches!(
        ops[1],
        RenderOp::FillRectangle(FillRectangle {
            ulx: 0.0,
            uly: 0.0,
            lrx: 2.0,
            lry: 1.0,
            fill_color: 0xf801_003f,
            cycle_type: CycleType::Fill,
            ..
        })
    ));
    assert!(matches!(ops[2], RenderOp::FullSync));
    assert!(decode_display_list_f3dex2(&rdram, 0x1000)
        .unwrap()
        .is_empty());
}


#[test]
fn setcombine_and_color_registers_are_snapshotted_on_triangles() {
    // Fail-against-bug: before this change all three commands fell into
    // the skip arm, so Triangle had no mode/primitive/environment state
    // and the rasterizer could only hardwire TEXEL0*SHADE.
    let mut rdram = vec![0u8; 0x4000];
    wr_vtx(&mut rdram, 0x3000, 2, 2, 0, [255, 255, 255, 255]);
    wr_vtx(&mut rdram, 0x3010, 12, 2, 0, [255, 255, 255, 255]);
    wr_vtx(&mut rdram, 0x3020, 7, 12, 0, [255, 255, 255, 255]);

    // Cycle 0 G_CC_BLENDI RGB: (ENV-SHADE)*TEXEL0+SHADE.
    // Alpha: TEXEL0*PRIMITIVE. Cycle 1 deliberately uses distinct
    // non-zero selectors in every field so a shifted/masked decode fails.
    let (cc0, cc1) = combine_cmd([5, 4, 1, 4], [1, 7, 3, 7], [3, 5, 11, 1], [5, 4, 6, 3]);
    let mut off = 0x1000;
    wr_cmd(&mut rdram, off, cc0, cc1);
    off += 8;
    wr_cmd(
        &mut rdram,
        off,
        ((G_SETPRIMCOLOR as u32) << 24) | 0x7f,
        0x11_22_33_44,
    );
    off += 8;
    wr_cmd(&mut rdram, off, (G_SETENVCOLOR as u32) << 24, 0xa0_b0_c0_d0);
    off += 8;
    wr_cmd(
        &mut rdram,
        off,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        0x3000,
    );
    off += 8;
    wr_cmd(
        &mut rdram,
        off,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    off += 8;
    wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

    let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
    assert_eq!(tris.len(), 1);
    let cc = tris[0].combiner;
    assert_eq!(cc.primitive, [0x11, 0x22, 0x33, 0x44]);
    assert_eq!(cc.environment, [0xa0, 0xb0, 0xc0, 0xd0]);
    assert_eq!(cc.prim_lod_fraction, 0x7f);
    assert_eq!(
        cc.mode.cycles[0].rgb,
        [
            ColorSource::Environment,
            ColorSource::Shade,
            ColorSource::Texel0,
            ColorSource::Shade,
        ]
    );
    assert_eq!(
        cc.mode.cycles[0].alpha,
        [
            AlphaSource::Texel0,
            AlphaSource::Zero,
            AlphaSource::Primitive,
            AlphaSource::Zero,
        ]
    );
    assert_eq!(
        cc.mode.cycles[1].rgb,
        [
            ColorSource::Primitive,
            ColorSource::Environment,
            ColorSource::ShadeAlpha,
            ColorSource::Texel0,
        ]
    );
    assert_eq!(
        cc.mode.cycles[1].alpha,
        [
            AlphaSource::Environment,
            AlphaSource::Shade,
            AlphaSource::PrimLodFraction,
            AlphaSource::Primitive,
        ]
    );
}


/// Fails against the pre-fix name-table-only decoder: several partial H/L
/// writes must merge without clobbering each other, and the resulting
/// cycle/filter/dither/alpha/coverage/Z/blender state plus blend-alpha
/// threshold must be snapshotted onto the emitted triangle.
#[test]
fn other_mode_partial_updates_are_decoded_and_carried_per_triangle() {
    let mut rdram = vec![0u8; 0x4000];
    wr_vtx(&mut rdram, 0x2000, 2, 2, 0, [255, 255, 255, 255]);
    wr_vtx(&mut rdram, 0x2010, 12, 2, 0, [255, 255, 255, 255]);
    wr_vtx(&mut rdram, 0x2020, 7, 12, 0, [255, 255, 255, 255]);

    let mut off = 0x1000;
    let mut emit = |w0: u32, w1: u32| {
        wr_cmd(&mut rdram, off, w0, w1);
        off += 8;
    };
    emit(((G_VTX as u32) << 24) | (3 << 12) | (3 << 1), 0x2000);
    emit(other_mode_cmd(G_SETOTHERMODE_H, 20, 2), 2 << 20); // G_CYC_COPY
    emit(other_mode_cmd(G_SETOTHERMODE_H, 12, 2), 2 << 12); // G_TF_BILERP
    emit(other_mode_cmd(G_SETOTHERMODE_H, 6, 2), 3 << 6); // G_CD_DISABLE
    emit(other_mode_cmd(G_SETOTHERMODE_H, 4, 2), 2 << 4); // G_AD_NOISE
    emit(other_mode_cmd(G_SETOTHERMODE_L, 0, 2), 1); // G_AC_THRESHOLD

    let blender = (1 << 30) | (2 << 26) | (3 << 22) | (2 << 28) | (1 << 24) | (3 << 16);
    let render = blender | 0x0010 | 0x0020 | 0x0100 | 0x0800 | 0x1000 | 0x2000 | 0x4000;
    emit(other_mode_cmd(G_SETOTHERMODE_L, 3, 29), render);
    emit((G_SETBLENDCOLOR as u32) << 24, 0x0102_0380);
    emit((G_TRI1 as u32) << 24 | (1 << 9) | (2 << 1), 0);
    emit(other_mode_cmd(G_SETOTHERMODE_L, 0, 2), 0); // G_AC_NONE
    emit((G_TRI1 as u32) << 24 | (1 << 9) | (2 << 1), 0);
    emit((G_ENDDL as u32) << 24, 0);

    let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
    assert_eq!(tris.len(), 2);
    let mode = tris[0].other_mode;
    assert_eq!(mode.cycle_type(), CycleType::Copy);
    assert_eq!(mode.texture_filter(), TextureFilter::Bilinear);
    assert_eq!(mode.rgb_dither(), RgbDither::Disabled);
    assert_eq!(mode.alpha_dither(), AlphaDither::Noise);
    assert_eq!(mode.alpha_compare(), AlphaCompare::Threshold);
    assert_eq!(mode.blend_color_alpha, 0x80);
    assert!(mode.depth_compare_enabled());
    assert!(mode.depth_update_enabled());
    assert_eq!(mode.coverage_destination(), CoverageDestination::Wrap);
    assert_eq!(mode.depth_mode(), DepthMode::Translucent);
    assert!(mode.coverage_times_alpha());
    assert!(mode.alpha_coverage_select());
    assert!(mode.force_blend());
    assert_eq!(
        mode.blender_cycle_1(),
        BlenderCycle {
            color_a: 1,
            alpha_a: 2,
            color_b: 3,
            alpha_b: 0,
        }
    );
    assert_eq!(
        mode.blender_cycle_2(),
        BlenderCycle {
            color_a: 2,
            alpha_a: 1,
            color_b: 0,
            alpha_b: 3,
        }
    );
    assert_eq!(tris[1].other_mode.alpha_compare(), AlphaCompare::None);
    assert_eq!(tris[1].other_mode.raw_high(), mode.raw_high());
    assert_eq!(tris[1].other_mode.raw_low(), mode.raw_low() & !3);
}


/// OoT's public G_RM_* constants embed G_AC_DITHER outside the nominal
/// gDPSetRenderMode bits-3..31 range. This fails if the decoder masks w1
/// instead of following the RSP/RT64 full-data OR behavior.
#[test]
fn render_mode_update_keeps_embedded_alpha_dither_bits() {
    let w0 = other_mode_cmd(G_SETOTHERMODE_L, 3, 29);
    let updated = update_other_mode_word(0, w0, 3 | 0x0010).unwrap();
    let mode = OtherMode::from_raw(0, updated, 0);
    assert_eq!(mode.alpha_compare(), AlphaCompare::Dither);
    assert!(mode.depth_compare_enabled());
}


/// Fails against the original decoder, which loudly skipped opcode 0xEF
/// and emitted every triangle with overwrite-only default state.
#[test]
fn full_othermode_command_snapshots_both_blender_cycles_on_triangle() {
    let mut rdram = vec![0u8; 0x4000];
    wr_vtx(&mut rdram, 0x2000, 2, 2, 0, [255, 255, 255, 128]);
    wr_vtx(&mut rdram, 0x2010, 12, 2, 0, [255, 255, 255, 128]);
    wr_vtx(&mut rdram, 0x2020, 7, 12, 0, [255, 255, 255, 128]);

    // G_CYC_2CYCLE plus the standard XLU tuple in both cycles:
    // IN*A_IN + MEM*(1-A). Selector positions are exactly GBL_c1/c2
    // (gbi.h:624-627); FORCE_BL is gbi.h:609.
    let high = 1 << 20;
    let low = (1 << 22) | (1 << 20) | 0x4000;
    let mut off = 0x1000;
    wr_cmd(
        &mut rdram,
        off,
        ((G_RDPSETOTHERMODE as u32) << 24) | high,
        low,
    );
    off += 8;
    wr_cmd(&mut rdram, off, (G_SETBLENDCOLOR as u32) << 24, 0x1020_3040);
    off += 8;
    wr_cmd(&mut rdram, off, (G_SETFOGCOLOR as u32) << 24, 0x5060_7080);
    off += 8;
    wr_cmd(
        &mut rdram,
        off,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        0x2000,
    );
    off += 8;
    wr_cmd(
        &mut rdram,
        off,
        ((G_TRI1 as u32) << 24) | (0 << 17) | (1 << 9) | (2 << 1),
        0,
    );
    off += 8;
    wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

    let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
    assert_eq!(tris.len(), 1);
    let blender = tris[0].blender;
    assert_eq!(blender.cycle_count, 2);
    assert!(blender.force_blend);
    assert_eq!(blender.blend_color, [0x10, 0x20, 0x30, 0x40]);
    assert_eq!(blender.fog_color, [0x50, 0x60, 0x70, 0x80]);
    for cycle in blender.cycles {
        assert_eq!(cycle.p, BlendColorInput::Combined);
        assert_eq!(cycle.a, BlendAlphaInput::Combined);
        assert_eq!(cycle.m, BlendColorInput::Framebuffer);
        assert_eq!(cycle.b, BlendBInput::OneMinusA);
    }
}


/// Partial setters use F3DEX2's inverted shift field, not the older F3D
/// direct shift. These exact words are what gDPSetCycleType and
/// gDPSetRenderMode emit through gSPSetOtherMode (gbi.h:3353-3369).
#[test]
fn partial_othermode_commands_patch_the_logical_bit_ranges() {
    let cycle_type_w0 = ((0xE3u32) << 24) | (10 << 8) | 1;
    let high = update_other_mode_word(0, cycle_type_w0, 1 << 20).unwrap();
    assert_eq!((high >> 20) & 3, 1);

    let render_mode_w0 = ((0xE2u32) << 24) | 28;
    let render_mode = (1 << 22) | (1 << 20) | 0x4000;
    let low = update_other_mode_word(0b101, render_mode_w0, render_mode).unwrap();
    assert_eq!(low & 0b111, 0b101, "bits below render mode stay intact");
    assert_eq!(low & !0b111, render_mode);
}


#[test]
fn setscissor_decodes_quarter_pixel_edges_on_emitted_triangle() {
    let mut rdram = vec![0u8; 0x4000];
    wr_vtx(&mut rdram, 0x2000, 2, 3, 0, [255, 0, 0, 255]);
    wr_vtx(&mut rdram, 0x2010, 12, 3, 0, [0, 255, 0, 255]);
    wr_vtx(&mut rdram, 0x2020, 2, 13, 0, [0, 0, 255, 255]);

    let raw_ulx = 5u32; // 1.25 px
    let raw_uly = 10u32; // 2.5 px
    let raw_lrx = 43u32; // 10.75 px
    let raw_lry = 48u32; // 12 px
    let mut off = 0x1000;
    wr_cmd(
        &mut rdram,
        off,
        ((G_SETSCISSOR as u32) << 24) | (raw_ulx << 12) | raw_uly,
        (1 << 25) | (1 << 24) | (raw_lrx << 12) | raw_lry,
    );
    off += 8;
    wr_cmd(
        &mut rdram,
        off,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        0x2000,
    );
    off += 8;
    wr_cmd(
        &mut rdram,
        off,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    off += 8;
    wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

    let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
    assert_eq!(tris.len(), 1);
    assert_eq!(
        tris[0].scissor,
        Some(ScissorRect {
            ulx: 1.25,
            uly: 2.5,
            lrx: 10.75,
            lry: 12.0,
            field: true,
            keep_odd: true,
        })
    );
}


// --- Perspective * view * model projection regression ----------------
//
// Fails against the pre-fix decoder, which transposed each `Mtx` on read
// AND accumulated the projection product in the wrong (proj-first) order.
// The two errors cancel for a single diagonal/symmetric matrix -- all the
// older f3dex2_replay fixture exercised -- so the bug slipped through, but
// for a real guPerspective * guLookAt * model chain the net composed MVP
// came out as the TRANSPOSE of the true one: clip `w` collapsed to tiny,
// sign-flipping values (~30, ~-13, ~-59) and the perspective divide flung
// a vertex that belongs near screen-center to ~800+ px off the 320x240
// screen. This test drives the exact live-OoT-gameplay P/V/M matrices
// through the full decode and asserts the vertex now lands on-screen with
// a coherent positive `w`.
#[test]
fn perspective_view_model_projects_vertex_on_screen() {
    let mut rdram = vec![0u8; 0x8000];

    // A CLEAN, self-consistent row-vector (N64 [row][col]) setup whose
    // on-screen anchors are derived INDEPENDENTLY, NOT reverse-engineered
    // to fit a transposed apply. guPerspective(fovy=60, aspect=4/3,
    // near=10, far=1000): projective term [2][3]=-1, depth translate at
    // [3][2]. The modelview is deliberately ASYMMETRIC (a 20° rotation
    // about Y + a translation to (30,-15,-120)) so `mvp != mvp^T` -- a
    // pure-translation/diagonal MVP would be transpose-invariant and could
    // NOT distinguish the bug from the fix.
    //
    // Under the CORRECT row-vector transform `clip = v · (M · P)`:
    //   - object origin (0,0,0) -> w=120, screen (211.96, 145.98);
    //   - vertex (10,20,0)      -> w=123.42, screen (226.35, 111.58).
    // The pre-fix COLUMN-vector apply (`(M·P)·v`) is the transpose: it
    // sends (10,20,0) to w=-9.9 (behind the camera) / px=(-42.7, 539.7),
    // i.e. off-screen -- the fanning-triangle bug.
    let persp = [
        [1.299038, 0.0, 0.0, 0.0],
        [0.0, 1.732051, 0.0, 0.0],
        [0.0, 0.0, -1.020202, -1.0],
        [0.0, 0.0, -20.202_02, 0.0],
    ];
    // Asymmetric modelview: rot(20° about Y) then translate(30,-15,-120),
    // in hardware [row][col] row-vector layout.
    let model = [
        [0.939693, 0.0, -0.342020, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.342020, 0.0, 0.939693, 0.0],
        [30.0, -15.0, -120.0, 1.0],
    ];

    // Triangle vertices (model space): center, +x/+y offset, +x.
    wr_vtx(&mut rdram, 0x3000, 0, 0, 0, [255, 0, 0, 255]);
    wr_vtx(&mut rdram, 0x3010, 10, 20, 0, [0, 255, 0, 255]);
    wr_vtx(&mut rdram, 0x3020, 20, 0, 0, [0, 0, 255, 255]);

    wr_mtx(&mut rdram, 0x2000, persp);
    wr_mtx(&mut rdram, 0x2200, model);
    wr_centered_viewport(&mut rdram, 0x2400);

    // G_MTX param bytes (wire = params ^ G_MTX_PUSH):
    //  perspective LOAD:  PROJECTION|LOAD   = 0x06 -> wire 0x07
    //  model LOAD:        LOAD (modelview)  = 0x02 -> wire 0x03
    let mtx_len = ((64u32 - 1) / 8) << 19;
    let mtx_cmd = |idx: u32| ((G_MTX as u32) << 24) | mtx_len | idx;
    let mut off = 0x1000;
    wr_cmd(&mut rdram, off, movemem_viewport_word(), 0x2400);
    off += 8;
    wr_cmd(&mut rdram, off, mtx_cmd(0x07), 0x2000); // persp LOAD
    off += 8;
    wr_cmd(&mut rdram, off, mtx_cmd(0x03), 0x2200); // model LOAD
    off += 8;
    wr_cmd(
        &mut rdram,
        off,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        0x3000,
    );
    off += 8;
    wr_cmd(
        &mut rdram,
        off,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    off += 8;
    wr_cmd(&mut rdram, off, (G_ENDDL as u32) << 24, 0);

    let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
    assert_eq!(tris.len(), 1, "expected one transformed triangle");
    // Explicit 320x240 viewport: NDC*160+160 / *120+120.
    // Independently-derived anchors (see numpy in the doc above):
    //   v0 (object origin) -> (211.96, 145.98), w=120.
    //   v1 (10,20,0)       -> (226.35, 111.58), w=123.42.
    let v0 = &tris[0].v[0];
    assert!(
        (v0.x - 211.96).abs() < 0.5 && (v0.y - 145.98).abs() < 0.5,
        "object-origin vertex must land at its independently-derived anchor \
         (~211.96, ~145.98) under the correct row-vector MVP; got ({}, {}) \
         (the transposed/column-vector apply misses it, off-screen)",
        v0.x,
        v0.y
    );
    let v1 = &tris[0].v[1];
    assert!(
        (v1.x - 226.35).abs() < 0.5 && (v1.y - 111.58).abs() < 0.5,
        "offset vertex drifted from the independently-derived on-screen \
         anchor (~226.35, ~111.58); got ({}, {}) -- a re-transpose sends it \
         to px≈(-42.7, 539.7), off-screen",
        v1.x,
        v1.y
    );
    // The sane depth is the load-bearing signal: w must be ~ -z_eye = 120,
    // never the pre-fix ±thousands sign-flipping garbage. (The transposed
    // apply gives v1 a NEGATIVE w=-9.9 -- behind the camera.)
    assert!(
        (v0.w - 120.0).abs() < 0.5,
        "clip-w must be the sane perspective depth ~120, got {}",
        v0.w
    );
}

// --- G_DL branch (gsSPBranchList) desync regression -----------------
//
// Fails against the pre-fix decoder: a G_DL with the NOPUSH (branch)
// flag used to recurse into the target and then CONTINUE decoding the
// parent stream. Because a branch's trailing bytes are not commands
// (here: raw garbage), the decoder walked into them and every byte
// became a bogus opcode -- the exact ~14K-junk-skip cascade seen on the
// real OoT gameplay task. After the fix a branch STOPS the parent stream.


#[test]
fn g_dl_branch_does_not_decode_bytes_after_the_branch() {
    // Layout:
    //   0x1000  parent DL: [G_DL NOPUSH -> 0x2000], then GARBAGE, G_ENDDL
    //   0x2000  target DL: [G_VTX(3) @ 0x3000], [G_TRI1 0,1,2], G_ENDDL
    //   0x3000  three vertices
    let mut rdram = vec![0u8; 0x4000];

    // Parent stream at 0x1000.
    // gsSPBranchList: w0 = G_DL<<24 | G_DL_NOPUSH<<16, w1 = target addr.
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_DL as u32) << 24) | (0x01 << 16),
        0x2000,
    );
    // "Garbage" right after the branch that the PRE-FIX decoder would
    // wrongly execute: a second VTX+TRI1 pair drawing a spurious extra
    // triangle. (In the real bug these trailing bytes were zero-fill /
    // an unrelated buffer that cascaded into ~14K junk-opcode skips; a
    // spurious *triangle* is the same "kept decoding after the branch"
    // fault, made observable as a hard count assertion.)
    wr_cmd(
        &mut rdram,
        0x1008,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        0x3000,
    );
    wr_cmd(
        &mut rdram,
        0x1010,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    wr_cmd(&mut rdram, 0x1018, (G_ENDDL as u32) << 24, 0);

    // Target stream at 0x2000: load 3 verts, draw 1 triangle, end.
    // G_VTX: n=3 in bits 12-19, end=3 in bits 1-7 -> v0 = end - n = 0.
    wr_cmd(
        &mut rdram,
        0x2000,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        0x3000,
    );
    // G_TRI1: three 7-bit slots at bits 17/9/1 -> slots 0,1,2.
    wr_cmd(
        &mut rdram,
        0x2008,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    wr_cmd(&mut rdram, 0x2010, (G_ENDDL as u32) << 24, 0);

    // Three vertices (raw screen coords; no transform loaded).
    wr_vtx(&mut rdram, 0x3000, 10, 10, 0, [255, 0, 0, 255]);
    wr_vtx(&mut rdram, 0x3010, 20, 10, 0, [0, 255, 0, 255]);
    wr_vtx(&mut rdram, 0x3020, 15, 20, 0, [0, 0, 255, 255]);

    // Segment 0 is identity here (addresses are already physical).
    let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();

    // Exactly the ONE triangle from the branched-to target -- no extra
    // garbage triangles, and (the real proof) no unrecognized-opcode
    // cascade from decoding the bytes after the branch. Pre-fix this
    // would have walked the 0x1008.. garbage as opcodes.
    assert_eq!(
        tris.len(),
        1,
        "branch must run the target then stop; got {} triangles \
         (pre-fix bug decoded post-branch garbage)",
        tris.len()
    );
    // The triangle carries the three planted vertex colors.
    assert_eq!(tris[0].v[0].r, 255);
    assert_eq!(tris[0].v[1].g, 255);
    assert_eq!(tris[0].v[2].b, 255);
}


#[test]
fn g_dl_call_resumes_parent_after_target() {
    // A CALL (G_DL_PUSH=0) must recurse AND resume the parent: parent
    // draws one tri, calls a sub-DL that draws one tri, then parent draws
    // a third after the call returns -> 3 triangles total.
    let mut rdram = vec![0u8; 0x4000];

    // Shared vertices at 0x3000 (0,1,2).
    wr_vtx(&mut rdram, 0x3000, 10, 10, 0, [255, 0, 0, 255]);
    wr_vtx(&mut rdram, 0x3010, 20, 10, 0, [0, 255, 0, 255]);
    wr_vtx(&mut rdram, 0x3020, 15, 20, 0, [0, 0, 255, 255]);

    let vtx = |rd: &mut [u8], off: usize| {
        wr_cmd(
            rd,
            off,
            ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            0x3000,
        );
    };
    let tri1 = |rd: &mut [u8], off: usize| {
        wr_cmd(rd, off, ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1), 0);
    };

    // Parent at 0x1000: VTX, TRI1, G_DL CALL -> 0x2000, TRI1, ENDDL.
    vtx(&mut rdram, 0x1000);
    tri1(&mut rdram, 0x1008);
    wr_cmd(&mut rdram, 0x1010, (G_DL as u32) << 24, 0x2000); // push=0 -> CALL
    tri1(&mut rdram, 0x1018);
    wr_cmd(&mut rdram, 0x1020, (G_ENDDL as u32) << 24, 0);

    // Sub-DL at 0x2000: VTX, TRI1, ENDDL.
    vtx(&mut rdram, 0x2000);
    tri1(&mut rdram, 0x2008);
    wr_cmd(&mut rdram, 0x2010, (G_ENDDL as u32) << 24, 0);

    let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
    assert_eq!(
        tris.len(),
        3,
        "call must resume the parent after the target returns"
    );
}


#[test]
fn g_dl_branch_chain_longer_than_call_stack_decodes_fully() {
    // A chain of 40 tail branches (gsSPBranchList) ending in a DL that
    // draws one triangle. On hardware a branch consumes NO return-stack
    // entry, so any chain length is legal. The pre-fix decoder recursed
    // per branch and counted it against MAX_DL_DEPTH, so a chain longer
    // than the cap silently dropped the tail (this exact "G_DL recursion
    // exceeded" warning fired on real OoT field frames).
    const CHAIN: usize = 40;
    let mut rdram = vec![0u8; 0x8000];

    wr_vtx(&mut rdram, 0x3000, 10, 10, 0, [255, 0, 0, 255]);
    wr_vtx(&mut rdram, 0x3010, 20, 10, 0, [0, 255, 0, 255]);
    wr_vtx(&mut rdram, 0x3020, 15, 20, 0, [0, 0, 255, 255]);

    // Links at 0x1000, 0x1010, 0x1020, ... each: [branch -> next], and
    // garbage would follow (nothing does -- a branch never returns).
    for i in 0..CHAIN {
        let at = 0x1000 + i * 0x10;
        let next = (0x1000 + (i + 1) * 0x10) as u32;
        wr_cmd(&mut rdram, at, ((G_DL as u32) << 24) | (0x01 << 16), next);
    }
    // Terminal DL after the last link: VTX, TRI1, ENDDL.
    let end = 0x1000 + CHAIN * 0x10;
    wr_cmd(
        &mut rdram,
        end,
        ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        0x3000,
    );
    wr_cmd(
        &mut rdram,
        end + 8,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    wr_cmd(&mut rdram, end + 16, (G_ENDDL as u32) << 24, 0);

    let tris = decode_display_list_f3dex2(&rdram, 0x1000).unwrap();
    assert_eq!(
        tris.len(),
        1,
        "a {CHAIN}-deep branch chain must reach its terminal DL \
         (branches consume no stack entry)"
    );
}


#[test]
#[should_panic(expected = "exceeded the 1048576-command budget")]
fn g_dl_cyclic_branch_traps_at_the_global_command_budget() {
    // A branch list that branches to ITSELF: hardware would spin
    // forever; the finite host decoder must identify the cycle instead
    // of returning a plausible empty render.
    let mut rdram = vec![0u8; 0x2000];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_DL as u32) << 24) | (0x01 << 16),
        0x1000,
    );
    let _ = decode_display_list_f3dex2(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "F3DEX2 display list is truncated at RDRAM 0x00001000")]
fn truncated_f3dex2_command_stream_traps_with_pc() {
    let rdram = vec![0u8; 0x1004];
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}
