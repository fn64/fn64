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
fn decode_ci8_uses_full_byte_as_rgba16_tlut_index() {
    let mut tlut = vec![[0, 0, 0, 0]; 256];
    tlut[0] = [1, 2, 3, 4];
    tlut[0x7f] = [5, 6, 7, 8];
    tlut[0xff] = [9, 10, 11, 12];
    assert_texture_row(
        &[0x00, 0x7f, 0xff],
        3,
        G_IM_FMT_CI,
        G_IM_SIZ_8B,
        0,
        tlut,
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
    );
}


#[test]
fn decode_ci4_combines_palette_bank_with_each_nibble() {
    let mut tlut = vec![[0, 0, 0, 0]; 0x30];
    tlut[0x21] = [1, 3, 5, 7];
    tlut[0x2f] = [2, 4, 6, 8];
    assert_texture_row(
        &[0x1f],
        2,
        G_IM_FMT_CI,
        G_IM_SIZ_4B,
        2,
        tlut,
        &[1, 3, 5, 7, 2, 4, 6, 8],
    );
}


#[test]
fn decode_ci4_pal16_load_uses_palette_local_indices() {
    // Fail-against-bug: G_LOADTLUT stores a 16-entry pal16 load in this
    // decoder as entries 0..15. The old CI4 arm added palette<<4 again,
    // indexed past this Vec for every nonzero bank, and returned magenta.
    let mut tlut = vec![[0, 0, 0, 0]; 16];
    tlut[1] = [11, 22, 33, 44];
    tlut[15] = [55, 66, 77, 88];
    assert_texture_row(
        &[0x1f],
        2,
        G_IM_FMT_CI,
        G_IM_SIZ_4B,
        2,
        tlut,
        &[11, 22, 33, 44, 55, 66, 77, 88],
    );
}

// --- Projection w-sign regression (the "giant triangles from a point"
//     bug): the MVP must be applied as a ROW vector, not a column vector.


/// Column-vector application of an asymmetric perspective·modelview MVP
/// (`mvp · v`) computes the TRANSPOSE of the true transform and produces a
/// huge, sign-flipping `w` -- the projection bug. `transform_point` must do
/// the row-vector product (`v · mvp`) so `w ≈ -z_eye`, a sane depth.
///
/// Matrices are the LIVE OoT gameplay task dump (decoded via `read_mtx`):
/// perspective P with the projective term in row 2 col 3, and a modelview
/// translation of (-53, -5, 0). Cited in `transform_point`'s doc comment.
#[test]
fn transform_point_row_vector_gives_sane_perspective_w() {
    // guPerspective output P (hardware [row][col], no transpose):
    let p: Mat4 = [
        [2.7990265, 0.0, 0.0, 0.0],
        [0.0, 3.7320404, 0.0, 0.0],
        [0.0, 0.0, -1.0015564, -1.0],
        [0.0, 0.0, -20.015625, 0.0],
    ];
    // Modelview: pure translation by (-53, -5, 0) (4th ROW, N64 layout).
    let mut m: Mat4 = identity();
    m[3][0] = -53.0;
    m[3][1] = -5.0;
    // mvp = modelview * (view*proj); here view is folded in, so M * P.
    let mvp = mat_mul(&m, &p);

    // Two object-space vertices of one small object (ob magnitudes ~10).
    // Under a correct row-vector transform their `w` is the SAME sane
    // depth (both at eye-z = -5 after the translate -> w = -z_eye = 5),
    // NOT the ±thousands sign-flipping garbage the transpose produced.
    for &(x, y, z) in &[(11.0, 0.0, -5.0), (5.0, 0.0, -5.0)] {
        let clip = transform_point(&mvp, x, y, z);
        let w = clip[3];
        // The true perspective depth for these verts is w = 5.0.
        assert!(
            (w - 5.0).abs() < 1e-3,
            "row-vector w should be the sane depth 5.0, got {w}"
        );
        assert!(w.abs() < 1e3, "w must be a small sane depth, got {w}");
    }

    // Guard against a regression to the column-vector form: assert the
    // BUGGED product (`mvp · v`, the old code) really did explode `w`.
    // This documents the failure mode so a reviewer sees the bug is real.
    let col_vec_w = {
        let v = [11.0f32, 0.0, -5.0, 1.0];
        // out[r] = sum_k mvp[r][k] * v[k] -- the OLD column-vector product.
        let mut s = 0.0;
        for k in 0..4 {
            s += mvp[3][k] * v[k];
        }
        s
    };
    assert!(
        col_vec_w.abs() > 1e3,
        "the column-vector (transposed) apply must produce the pathological \
         large w this test guards against; got {col_vec_w}"
    );
    // And it flips sign vs the second vertex (the "fan" signature).
    let col_vec_w2 = {
        let v = [5.0f32, 0.0, -5.0, 1.0];
        let mut s = 0.0;
        for k in 0..4 {
            s += mvp[3][k] * v[k];
        }
        s
    };
    assert!(
        col_vec_w.signum() == col_vec_w2.signum() && col_vec_w != col_vec_w2,
        "column-vector w varies wildly with x (the bug); w1={col_vec_w} w2={col_vec_w2}"
    );
}


/// A symmetric/diagonal matrix (all the reference fixtures) is unchanged
/// by the row-vs-column swap (`m == m^T`), so the fix is transparent to
/// the byte-exact goldens.
#[test]
fn transform_point_symmetric_matrix_unaffected_by_convention() {
    let mut m: Mat4 = identity();
    m[0][0] = 2.0;
    m[1][1] = 3.0;
    m[2][2] = 4.0;
    m[3][3] = 1.0;
    let clip = transform_point(&m, 5.0, 7.0, 9.0);
    assert_eq!(clip, [10.0, 21.0, 36.0, 1.0]);
}


/// Regression for the exact fixed-point `guLookAt` matrix observed in the
/// Hyrule Field title-camera task. The writer trace establishes that
/// `guLookAtF` receives eye `(-4000,-1,5228)`; its translation therefore
/// is `(3263,694,5675) = -(eye · basis)`. Those translation values are
/// camera-space coordinates of the world origin, not the world-space eye.
///
/// Replacing them with `-translation · basis` (the discarded diagnostic
/// transform) moves the camera to a different world-space eye. This test
/// fails under that rewrite because the traced eye no longer maps to the
/// view-space origin.
#[test]
fn hyrule_field_live_gu_look_at_translation_matches_traced_eye() {
    // Decoded from the 64-byte Mtx written at physical 0x1888c8. The
    // fixed-point quantization accounts for the small origin tolerance.
    let view: Mat4 = [
        [-0.3885498, 0.11167908, 0.9146271, 0.0],
        [-1.5258789e-5, 0.99261475, -0.12121582, 0.0],
        [-0.92141724, -0.04710388, -0.38568115, 0.0],
        [3262.9912, 694.052, 5674.783, 1.0],
    ];
    let eye = [-4000.0, -1.0, 5228.0];

    for (c, (((&basis_x, &basis_y), &basis_z), &translation)) in view[0]
        .iter()
        .zip(view[1].iter())
        .zip(view[2].iter())
        .zip(view[3].iter())
        .take(3)
        .enumerate()
    {
        let expected_translation = -(eye[0] * basis_x + eye[1] * basis_y + eye[2] * basis_z);
        assert!(
            (translation - expected_translation).abs() < 0.1,
            "translation[{c}] must be -(eye · basis[{c}]): got {}, expected {expected_translation}",
            translation
        );
    }

    let eye_in_view = transform_point(&view, eye[0], eye[1], eye[2]);
    for (axis, value) in eye_in_view[..3].iter().enumerate() {
        assert!(
            value.abs() < 0.1,
            "traced eye must map to the view-space origin; axis {axis} was {value}"
        );
    }
    assert!((eye_in_view[3] - 1.0).abs() < f32::EPSILON);
}


// --- Synthetic large-world projection regression ---------------------
//
// This synthetic scene has a camera at world ~(3000,700,5600) and an
// object translated to ~-4000, so both sides carry LARGE world
// coordinates. It drives the full decode path -- fixed-point `Mtx`
// bytes (`read_mtx`) -> projection LOAD(persp) then PROJECTION|MUL(view)
// -> modelview LOAD -> `recompute_mvp` (`M · (V · P)`) -> row-vector
// `transform_point` -> an explicit 320x240 viewport map -- for the exact
// large-world matrix shapes and asserts every vertex lands in-frustum
// with a sane POSITIVE `w` (~ -z_eye ~= +7000), never the negative /
// sign-flipping `w` and ±4000 screen-z of the mis-projection.
//
// The synthetic view is a proper `guLookAt` matrix: its translation row is
// `-(eye · basis)` = (5419.7, -367.3, -3367.7), NOT the raw eye. That is
// the load-bearing distinction -- feed the raw eye (3000,700,5600) into
// row 3 instead and the origin vertex flips to `w = -1921` (behind the
// camera). This asserts the decode+compose of a correct synthetic
// large-world view/model/perspective chain.
//
// It fails against the historical transpose bug too: a re-introduced
// `Mtx` transpose-on-read or a column-vector apply turns the asymmetric
// large-world MVP into its transpose and collapses `w` to garbage.
#[test]
fn large_world_perspective_view_model_projects_in_frustum() {
    let mut rdram = vec![0u8; 0x8000];

    // guPerspective(fovy=60, aspect=4/3, near=100, far=12800), hardware
    // [row][col]: projective term [2][3]=-1, depth translate [3][2].
    let persp = [
        [1.299_038, 0.0, 0.0, 0.0],
        [0.0, 1.7320508, 0.0, 0.0],
        [0.0, 0.0, -1.015_748, -1.0],
        [0.0, 0.0, -201.574_8, 0.0],
    ];
    // PROPER guLookAt view: 3x3 = camera basis (right/up/look as columns),
    // translation ROW = -(eye · basis). Eye world ~(3000,700,5600) looking
    // toward (-4000,0,5200). (Raw eye in row 3 would be the bug.)
    let view = [
        [0.05704979, -0.09918146, 0.993_432_6, 0.0],
        [0.0, 0.99505322, 0.09934326, 0.0],
        [-0.998_371_3, -0.00566751, 0.05676758, 0.0],
        [5_419.73, -367.254_8, -3367.7366, 1.0],
    ];
    // Large-world object modelview: rot(15° about Y) then translate to
    // world (-4000, 0, 5200) -- asymmetric so `mvp != mvp^T`.
    let model = [
        [0.965_925_8, 0.0, -0.25881905, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.25881905, 0.0, 0.965_925_8, 0.0],
        [-4000.0, 0.0, 5200.0, 1.0],
    ];

    // Object-space vertices (small, ob magnitudes ~50).
    wr_vtx(&mut rdram, 0x3000, 0, 0, 0, [255, 0, 0, 255]);
    wr_vtx(&mut rdram, 0x3010, 50, 30, 0, [0, 255, 0, 255]);
    wr_vtx(&mut rdram, 0x3020, -50, 0, 40, [0, 0, 255, 255]);

    wr_mtx(&mut rdram, 0x2000, persp);
    wr_mtx(&mut rdram, 0x2100, view);
    wr_mtx(&mut rdram, 0x2200, model);
    wr_centered_viewport(&mut rdram, 0x2400);

    // G_MTX wire = params ^ G_MTX_PUSH:
    //   persp PROJECTION|LOAD        = 0x06 -> wire 0x07
    //   view  PROJECTION|MUL(NOPUSH) = 0x04 -> wire 0x05
    //   model LOAD (modelview)       = 0x02 -> wire 0x03
    let mtx_len = ((64u32 - 1) / 8) << 19;
    let mtx_cmd = |idx: u32| ((G_MTX as u32) << 24) | mtx_len | idx;
    let mut off = 0x1000;
    wr_cmd(&mut rdram, off, movemem_viewport_word(), 0x2400);
    off += 8;
    wr_cmd(&mut rdram, off, mtx_cmd(0x07), 0x2000); // persp LOAD
    off += 8;
    wr_cmd(&mut rdram, off, mtx_cmd(0x05), 0x2100); // view PROJECTION|MUL
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

    // Every vertex must project to a sane POSITIVE depth ~7000 and land
    // inside the explicit 320x240 screen ([0,320] x [0,240]). The origin
    // vertex maps to screen center (~160, 120). The mis-projection gave
    // negative `w` and NDC well outside [-1,1] (pz swinging ±4000).
    for (i, v) in tris[0].v.iter().enumerate() {
        assert!(
            v.w > 1.0,
            "large-world vertex {i} must have a sane positive clip-w \
             (~7000, = -z_eye), got w={} (negative/tiny w is the \
             mis-projection this test guards)",
            v.w
        );
        assert!(
            (5000.0..9000.0).contains(&v.w),
            "large-world clip-w must be the coherent perspective depth \
             (~7000), got w={} -- a decode transpose / wrong MVP order \
             turns it into garbage",
            v.w
        );
        assert!(
            (0.0..=320.0).contains(&v.x) && (0.0..=240.0).contains(&v.y),
            "large-world vertex {i} must land inside the 320x240 screen, \
             got ({}, {}) -- out-of-frustum is the ±4000 pz mis-projection",
            v.x,
            v.y
        );
    }
    // Origin vertex at screen center is the crisp anchor.
    let v0 = &tris[0].v[0];
    assert!(
        (v0.x - 160.0).abs() < 1.0 && (v0.y - 120.0).abs() < 1.0,
        "object-origin vertex must land at screen center (~160, ~120) \
         under the correct large-world MVP; got ({}, {})",
        v0.x,
        v0.y
    );
}


#[test]
fn raw_rdp_range_rejects_truncated_edge_triangle_by_name() {
    let mut rdram = vec![0u8; 0x20];
    wr_cmd(&mut rdram, 0, 0x0800_0000, 0);
    let error = validate_raw_rdp_command_range(&rdram, 0, 8).unwrap_err();
    assert!(error.to_string().contains("truncated"));
    assert!(error.to_string().contains("RDP_TRI_FILL"));
}


#[test]
fn raw_rdp_full_sync_inspection_distinguishes_absent_and_reached() {
    let mut rdram = vec![0u8; 16];
    wr_cmd(&mut rdram, 0, (G_RDPPIPESYNC as u32) << 24, 0);
    assert_eq!(
        raw_rdp_full_sync_status(&rdram, 0, 8).unwrap(),
        fn64_render::RawRdpScan::Complete(fn64_render::DpFullSyncStatus::NotReached)
    );

    wr_cmd(&mut rdram, 8, (G_RDPFULLSYNC as u32) << 24, 0x1234_5678);
    assert_eq!(
        raw_rdp_full_sync_status(&rdram, 0, 16).unwrap(),
        fn64_render::RawRdpScan::Complete(fn64_render::DpFullSyncStatus::Reached)
    );
}


#[test]
fn raw_rdp_full_sync_inspection_skips_triangle_payload_words() {
    let mut rdram = vec![0u8; 32];
    wr_cmd(&mut rdram, 0, 0x0800_0000, 0);
    wr_cmd(&mut rdram, 8, (G_RDPFULLSYNC as u32) << 24, 0);

    assert_eq!(
        raw_rdp_full_sync_status(&rdram, 0, 32).unwrap(),
        fn64_render::RawRdpScan::Complete(fn64_render::DpFullSyncStatus::NotReached),
        "an opcode-shaped triangle coefficient is data, not a command"
    );
}


#[test]
fn raw_rdp_full_sync_inspection_reports_a_truncated_command_as_incomplete() {
    // A known-width command that overruns the range is NOT an error: the DPC
    // accepts END extensions in 8-byte increments, so a multiword command
    // straddles several END writes and hardware stalls CURRENT at its start until
    // the rest arrives. This test previously demanded an error, which is the
    // defect -- the caller could not tell "wait for more bytes" from "this is
    // malformed", and the raw CPU MMIO ingress panicked on a legal sequence.
    let mut rdram = vec![0u8; 8];
    wr_cmd(&mut rdram, 0, 0x0800_0000, 0);
    let scanned = raw_rdp_full_sync_status(&rdram, 0, 8)
        .expect("a truncated tail is a stall, not a rejection");
    let fn64_render::RawRdpScan::Incomplete {
        command_start,
        bytes_required,
        bytes_available,
        ..
    } = scanned
    else {
        panic!("an 8-byte range holding a 32-byte command must scan Incomplete");
    };
    assert_eq!(command_start, 0);
    // opcode 0x08 is a base triangle: 32 bytes.
    assert_eq!(bytes_required, 32);
    assert_eq!(bytes_available, 8);
}


/// RDP command ids 0x01..=0x07 are the rest of the No Operation block that
/// 0x00 opens. WCW/nWo Revenge's microcode emits one; WM2000's never does,
/// which is why the omission survived until a second title booted.
#[test]
fn raw_rdp_low_no_operation_block_is_accepted_and_one_word_wide() {
    for opcode in 0x01u8..=0x07 {
        let mut rdram = vec![0u8; 16];
        wr_cmd(&mut rdram, 0, (opcode as u32) << 24, 0);
        wr_cmd(&mut rdram, 8, (G_RDPFULLSYNC as u32) << 24, 0);

        validate_raw_rdp_command_range(&rdram, 0, 16).unwrap_or_else(|error| {
            panic!("RDP No Operation {opcode:#04x} must validate: {error}")
        });
        assert_eq!(raw_rdp_command_width(opcode), Some(8));
        assert_eq!(
            raw_rdp_full_sync_status(&rdram, 0, 16).unwrap(),
            fn64_render::RawRdpScan::Complete(fn64_render::DpFullSyncStatus::Reached),
            "No Operation {opcode:#04x} must advance exactly one command word"
        );
    }
}

/// The No Operation command table marks every bit don't-care except
/// `command[5:0]` at 61:56 -- all of word 1 and bits 55:0 of word 0 are
/// "--", and the command stalls the pipeline for a cycle without consuming
/// an input. WCW/nWo Revenge submits 0x000a0000 in those bits; hardware
/// ignores it, so the raw lane must not assert on it.
#[test]
fn raw_rdp_no_operation_admits_an_unassigned_payload() {
    let mut rdram = vec![0u8; 32];
    wr_cmd(&mut rdram, 0, 0x000a_0000, 0xdead_beef);
    wr_cmd(&mut rdram, 8, (G_RDPFULLSYNC as u32) << 24, 0);
    wr_cmd(&mut rdram, 16, (G_ENDDL as u32) << 24, 0);
    let ops = decode_raw_rdp_ops(&rdram, 0).unwrap();
    assert!(
        ops.iter().any(|op| matches!(op, RenderOp::FullSync)),
        "the scan must continue through a payload-bearing No Operation"
    );
}

/// The same don't-care argument the sync commands already made for their
/// second word covers their first word's bits 55:0 too.
#[test]
fn raw_rdp_sync_commands_admit_unassigned_first_word_bits() {
    for opcode in [G_RDPLOADSYNC, G_RDPPIPESYNC, G_RDPTILESYNC, G_RDPFULLSYNC] {
        let mut rdram = vec![0u8; 24];
        wr_cmd(&mut rdram, 0, ((opcode as u32) << 24) | 0x000a_0000, 0xdead_beef);
        wr_cmd(&mut rdram, 8, (G_ENDDL as u32) << 24, 0);
        decode_raw_rdp_ops(&rdram, 0).unwrap_or_else(|error| {
            panic!("raw {opcode:#04x} must admit unassigned payload bits: {error}")
        });
    }
}

/// The exemption is raw-lane only. F3DEX2 macros generate zero here, so the
/// geometry lane keeps checking -- a tagged GBI no-op still traps.
#[test]
#[should_panic(expected = "G_NOOP reserved first-word payload must be zero")]
fn gbi_lane_still_rejects_a_nonzero_noop_first_word() {
    let mut rdram = vec![0u8; 0x1010];
    wr_cmd(&mut rdram, 0x1000, ((G_NOOP as u32) << 24) | 0x000a_0000, 0);
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}

/// Bits 63:62 of the top wire byte are don't-care, so a command may arrive
/// under any of four spellings. WCW/nWo Revenge emits Set Color Image as
/// 0x7f where the GBI macros emit 0xff, and was rejected mid-frame for it.
#[test]
fn raw_rdp_accepts_every_prefix_spelling_of_a_command() {
    for prefix in [0x00u8, 0x40, 0x80, 0xc0] {
        assert_eq!(
            canonical_raw_rdp_opcode(prefix | 0x3f),
            G_SETCIMG,
            "Set Color Image spelled {:#04x} must canonicalize to G_SETCIMG",
            prefix | 0x3f
        );
        // Sync Full must stay recognizable under every spelling too: an
        // unrecognized one is a DP completion interrupt never raised.
        let mut rdram = vec![0u8; 8];
        wr_cmd(&mut rdram, 0, ((prefix | 0x29) as u32) << 24, 0);
        validate_raw_rdp_command_range(&rdram, 0, 8).unwrap();
        assert_eq!(
            raw_rdp_full_sync_status(&rdram, 0, 8).unwrap(),
            fn64_render::RawRdpScan::Complete(fn64_render::DpFullSyncStatus::Reached)
        );
    }
    // The triangles keep their bare spelling, since the decoder matches them
    // on the 6-bit command rather than a GBI constant.
    assert_eq!(canonical_raw_rdp_opcode(0xcf), 0x0f);
    assert_eq!(canonical_raw_rdp_opcode(0x0f), 0x0f);
}

#[test]
fn raw_rdp_unknown_opcode_records_returned_error() {
    fn64_runtime::arm_unsupported_events(None).unwrap();
    let mut rdram = vec![0u8; 8];
    wr_cmd(&mut rdram, 0, 0x1000_0000, 0);

    let error = validate_raw_rdp_command_range(&rdram, 0, 8).unwrap_err();
    assert!(error.to_string().contains("G_<unrecognized>"));
    let events = fn64_runtime::copy_unsupported_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].operation, "render.gbi.raw-rdp.command");
    assert_eq!(
        events[0].disposition,
        fn64_runtime::UnsupportedDisposition::ReturnedError
    );
    // 0x10 is in the No Operation region this decoder deliberately does not
    // accept. The diagnostic must carry the wire byte as submitted, not only
    // the canonical spelling (0xd0), or a reader cannot find it in the stream.
    assert!(events[0].context.contains("0x10"));
    assert!(events[0].context.contains("wire byte 0x10"));
}


#[test]
fn raw_rdp_sync_commands_ignore_their_unassigned_second_word() {
    let mut rdram = vec![0u8; 40];
    for (index, opcode) in [G_RDPLOADSYNC, G_RDPPIPESYNC, G_RDPTILESYNC, G_RDPFULLSYNC]
        .into_iter()
        .enumerate()
    {
        wr_cmd(
            &mut rdram,
            index * 8,
            (opcode as u32) << 24,
            0x0616_181a_u32.wrapping_add(index as u32),
        );
    }
    wr_cmd(&mut rdram, 32, (G_ENDDL as u32) << 24, 0);

    validate_raw_rdp_command_range(&rdram, 0, 32).unwrap();
    let ops = decode_raw_rdp_ops(&rdram, 0).unwrap();
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], RenderOp::FullSync));
}


#[test]
fn raw_rdp_edge_triangle_retains_signed_coefficients_through_render_op() {
    let mut rdram = vec![0u8; 40];
    let yh = 4;
    let ym = 4 * 4;
    let yl = 7 * 4;
    let slope_major = (5.0f32 / 6.0 * 65536.0).round() as u32;
    let slope_low = (5.0f32 / 3.0 * 65536.0).round() as u32;
    wr_cmd(
        &mut rdram,
        0,
        0x0800_0000 | (1 << 23) | (2 << 19) | (3 << 16) | yl,
        (ym << 16) | yh,
    );
    wr_cmd(&mut rdram, 8, 1 << 16, slope_low);
    wr_cmd(&mut rdram, 16, 1 << 16, slope_major);
    wr_cmd(&mut rdram, 24, 1 << 16, 0);
    wr_cmd(&mut rdram, 32, (G_ENDDL as u32) << 24, 0);

    validate_raw_rdp_command_range(&rdram, 0, 32).unwrap();
    let coefficients = decode_rdp_edge_coefficients(&rdram, 0).unwrap();
    assert!(coefficients.left_major);
    assert_eq!((coefficients.level, coefficients.tile), (2, 3));
    assert_eq!(
        (coefficients.yh, coefficients.ym, coefficients.yl),
        (4, 16, 28)
    );
    let ops = decode_raw_rdp_ops(&rdram, 0).unwrap();
    let RenderOp::RawTriangle(triangle) = &ops[0] else {
        panic!("raw edge command did not emit a raw triangle")
    };
    assert_eq!(triangle.edge, coefficients);
    assert!(triangle.shade.is_none());
    assert!(triangle.texture_coefficients.is_none());
    assert!(triangle.z.is_none());
}


#[test]
fn raw_rdp_z_triangle_retains_all_depth_coefficients() {
    let mut rdram = vec![0u8; 56];
    let yh = 4;
    let ym = 4 * 4;
    let yl = 7 * 4;
    wr_cmd(&mut rdram, 0, 0x0980_0000 | yl, (ym << 16) | yh);
    wr_cmd(
        &mut rdram,
        8,
        1 << 16,
        (5.0f32 / 3.0 * 65536.0).round() as u32,
    );
    wr_cmd(
        &mut rdram,
        16,
        1 << 16,
        (5.0f32 / 6.0 * 65536.0).round() as u32,
    );
    wr_cmd(&mut rdram, 24, 1 << 16, 0);
    wr_cmd(&mut rdram, 32, 2 << 16, 1 << 16);
    wr_cmd(&mut rdram, 40, 3 << 16, 0);
    wr_cmd(&mut rdram, 48, (G_ENDDL as u32) << 24, 0);

    validate_raw_rdp_command_range(&rdram, 0, 48).unwrap();
    assert_eq!(
        decode_rdp_z_coefficients(&rdram, 32),
        Some(RdpZCoefficients {
            z: 2 << 16,
            dzdx: 1 << 16,
            dzde: 3 << 16,
            dzdy: 0,
        })
    );
    let ops = decode_raw_rdp_ops(&rdram, 0).unwrap();
    let RenderOp::RawTriangle(triangle) = &ops[0] else {
        panic!("raw edge-plus-Z command did not emit a raw triangle")
    };
    assert_eq!(
        triangle.z,
        Some(RdpZCoefficients {
            z: 2 << 16,
            dzdx: 1 << 16,
            dzde: 3 << 16,
            dzdy: 0,
        })
    );
}


#[test]
fn raw_rdp_shade_z_triangle_decodes_signed_component_gradients() {
    let mut rdram = vec![0u8; 120];
    let yh = 4;
    let ym = 4 * 4;
    let yl = 7 * 4;
    wr_cmd(&mut rdram, 0, 0x0d80_0000 | yl, (ym << 16) | yh);
    wr_cmd(
        &mut rdram,
        8,
        1 << 16,
        (5.0f32 / 3.0 * 65536.0).round() as u32,
    );
    wr_cmd(
        &mut rdram,
        16,
        1 << 16,
        (5.0f32 / 6.0 * 65536.0).round() as u32,
    );
    wr_cmd(&mut rdram, 24, 1 << 16, 0);
    wr_cmd(&mut rdram, 32, (10 << 16) | 20, (30 << 16) | 255);
    wr_cmd(&mut rdram, 40, u32::from(u16::MAX) << 16, 0);
    wr_cmd(&mut rdram, 48, 0, 0);
    wr_cmd(&mut rdram, 56, 0, 0);
    wr_cmd(&mut rdram, 64, 0, 0);
    wr_cmd(&mut rdram, 72, 2, 0);
    wr_cmd(&mut rdram, 80, 0, 0);
    wr_cmd(&mut rdram, 88, 0, 0);
    wr_cmd(&mut rdram, 96, 4 << 16, 0);
    wr_cmd(&mut rdram, 104, 0, 0);
    wr_cmd(&mut rdram, 112, (G_ENDDL as u32) << 24, 0);

    validate_raw_rdp_command_range(&rdram, 0, 112).unwrap();
    let shade = decode_rdp_shade_coefficients(&rdram, 32).unwrap();
    assert_eq!(shade.color, [10 << 16, 20 << 16, 30 << 16, 255 << 16]);
    assert_eq!(shade.dcdx, [-65536, 0, 0, 0]);
    assert_eq!(shade.dcdy, [0, 2 << 16, 0, 0]);
    let ops = decode_raw_rdp_ops(&rdram, 0).unwrap();
    let RenderOp::RawTriangle(triangle) = &ops[0] else {
        panic!("raw shade command did not emit a raw triangle")
    };
    assert_eq!(triangle.shade, Some(shade));
    assert_eq!(
        triangle.z,
        Some(RdpZCoefficients {
            z: 4 << 16,
            dzdx: 0,
            dzde: 0,
            dzdy: 0,
        })
    );
}


#[test]
fn raw_rdp_texture_coefficients_preserve_signed_fixed_components() {
    let mut rdram = vec![0u8; 64];
    wr_cmd(&mut rdram, 0, (u32::from(u16::MAX - 1) << 16) | 3, 1 << 16);
    wr_cmd(&mut rdram, 8, (1 << 16) | u32::from(u16::MAX), 0);
    wr_cmd(&mut rdram, 16, 0x8000_0000, 0);
    wr_cmd(&mut rdram, 24, 0, 0);
    wr_cmd(&mut rdram, 32, 0, 0);
    wr_cmd(&mut rdram, 40, 0, 0);
    wr_cmd(&mut rdram, 48, 0, 0);
    wr_cmd(&mut rdram, 56, 0, 0);

    assert_eq!(
        decode_rdp_texture_coefficients(&rdram, 0),
        Some(RdpTextureCoefficients {
            stw: [-(2 << 16) + 0x8000, 3 << 16, 1 << 16],
            dstdx: [1 << 16, -65536, 0],
            dstde: [0; 3],
            dstdy: [0; 3],
        })
    );
}


#[test]
fn raw_rdp_triangle_widths_cover_every_coefficient_variant() {
    assert_eq!(
        (0x08..=0x0f)
            .map(|opcode| raw_rdp_command_width(opcode).unwrap())
            .collect::<Vec<_>>(),
        [32, 48, 96, 112, 96, 112, 160, 176]
    );
}


#[test]
fn raw_rdp_triangle_accepts_flagged_six_bit_wire_opcode() {
    let mut rdram = vec![0u8; 176];
    wr_cmd(&mut rdram, 0, 0xcf00_0000, 0);

    validate_raw_rdp_command_range(&rdram, 0, 176).unwrap();
    assert_eq!(raw_rdp_command_width(0xcf), Some(176));
    assert_eq!(raw_rdp_opcode_name(0xcf & 0x3f), "RDP_TRI_SHADE_TXTR_ZBUFF");
}


#[test]
fn raw_rdp_range_accepts_depth_image_register_command() {
    let mut rdram = vec![0u8; 8];
    wr_cmd(&mut rdram, 0, 0xfe00_0000, 0x0000_0400);
    validate_raw_rdp_command_range(&rdram, 0, 8).unwrap();
}
