// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use fn64_render::{
    GeometryUcodeProfile, UcodeId,
};
use sha2::Digest;
use std::fmt::Write as _;
use super::*;
use super::wire::*;
use super::types::*;
use super::matrix::*;
use super::state::*;

/// Derive the per-triangle [`CullMode`] from the current F3DEX2 geometry
/// mode's `G_CULL_FRONT`/`G_CULL_BACK` bits (`F3DEX2-CONCEPTS.md` §2.4).
pub(super) fn cull_mode_from(geometry_mode: u32) -> CullMode {
    let front = geometry_mode & G_CULL_FRONT != 0;
    let back = geometry_mode & G_CULL_BACK != 0;
    match (front, back) {
        (true, true) => CullMode::Both,
        (true, false) => CullMode::Front,
        (false, true) => CullMode::Back,
        (false, false) => CullMode::None,
    }
}

/// Apply one F3DEX2 partial other-mode update. Returns `None` only for a
/// malformed range that cannot fit in a 32-bit H/L word.
pub(super) fn update_other_mode_word(current: u32, w0: u32, data: u32) -> Option<u32> {
    let length = (w0 & 0xff) + 1;
    let encoded_shift = (w0 >> 8) & 0xff;
    let shift = 32u32.checked_sub(encoded_shift.checked_add(length)?)?;
    if length > 32 {
        return None;
    }
    let mask = if length == 32 {
        u32::MAX
    } else {
        (((1u64 << length) - 1) << shift) as u32
    };
    // Deliberately OR the complete data word, as RT64 does. Public gbi.h's
    // predefined render modes include G_AC_DITHER in bits 0..1 even though
    // gDPSetRenderMode requests the nominal bits-3..31 range
    // (`gbi.h:700-702,756-758,802-804,824-827,3484-3487`). Masking `data`
    // here would erase that alpha-compare mode from real OoT display lists.
    Some((current & !mask) | data)
}

/// The texture to bind to triangles emitted right now. `None` means
/// `G_TEXTURE` disabled texturing; enabling it without a live TMEM image is a
/// named failure rather than a white/flat-shaded substitution.
pub(super) fn texture_for_tile(
    tex: &TexState,
    tile_index: u8,
    texture_lut: u8,
    storage: &std::rc::Rc<Tmem>,
) -> Option<Texture> {
    let index = usize::from(tile_index);
    let tile = tex.tiles[index];
    if !tex.tmem.is_initialized() {
        return None;
    }
    // Programming Manual Chapter 13, "Tile Attributes": LRS/LRT are used
    // only for clamping. A wrapped axis is valid even when its unsigned
    // origin sits near 1024 and its unused upper clamp bound is numerically
    // lower; its address domain comes from the mask instead.
    let axis_dimension = |low: u16, high: u16, clamp: bool, mask: u8| {
        if mask != 0 && !clamp {
            Some(1_u32 << mask)
        } else {
            let low = low / 4;
            let high = high / 4;
            (high >= low).then(|| u32::from(high - low + 1))
        }
    };
    let width = axis_dimension(tile.uls, tile.lrs, tile.clamp_s, tile.mask_s)?;
    let height = axis_dimension(tile.ult, tile.lrt, tile.clamp_t, tile.mask_t)?;
    if width > 1024 || height > 1024 {
        return None;
    }
    Some(Texture {
        format: tile.fmt,
        size: tile.siz,
        width,
        height,
        texels: std::rc::Rc::new(Vec::new()),
        clamp_s: tile.clamp_s,
        clamp_t: tile.clamp_t,
        mirror_s: tile.mirror_s,
        mirror_t: tile.mirror_t,
        mask_s: tile.mask_s,
        mask_t: tile.mask_t,
        shift_s: tile.shift_s,
        shift_t: tile.shift_t,
        origin_s: tile.uls as f32 / 4.0,
        origin_t: tile.ult as f32 / 4.0,
        tmem: Some(std::rc::Rc::new(TmemTexture {
            storage: storage.clone(),
            tile,
            texture_lut,
        })),
        lod: None,
    })
}

pub(super) fn bind_texture_set(
    tex: &TexState,
    primitive_tile: u8,
    max_level: u8,
    texture_lut: u8,
) -> Option<Texture> {
    let storage = tex.tmem.clone();
    let tiles =
        std::array::from_fn(|tile| texture_for_tile(tex, tile as u8, texture_lut, &storage));
    Some(
        tiles[usize::from(primitive_tile)]
            .clone()?
            .with_lod_snapshot(tiles, primitive_tile, max_level),
    )
}

pub(super) fn active_texture(tex: &TexState, other_mode: OtherMode) -> Option<Texture> {
    if tex.tex_enabled {
        Some(
            bind_texture_set(
                tex,
                tex.tex_tile,
                tex.tex_max_level,
                other_mode.texture_lut(),
            )
            .unwrap_or_else(|| {
                let tile = tex.tiles[usize::from(tex.tex_tile)];
                panic!(
                    "G_TEXTURE enables tile {} but no initialized TMEM image with a valid G_SETTILESIZE extent exists: ({}, {})..({}, {})",
                    tex.tex_tile, tile.uls, tile.ult, tile.lrs, tile.lrt
                )
            }),
        )
    } else {
        None
    }
}

pub(super) fn active_blender(state: &DecodeState) -> BlenderState {
    BlenderState::from_other_mode(
        state.other_mode.raw_low(),
        state.other_mode.raw_high(),
        state.blend_color,
        state.fog_color,
    )
}

/// Recompute the cached model-view-projection matrix from the current stack.
///
/// `state.proj` already holds the accumulated `view * proj` product (built
/// left-multiplied in the G_MTX handler, hardware order). The full transform
/// is `mvp = modelview * (view * proj)` = `M * V * P`, kept in hardware
/// `[row][col]` layout. The incoming vertex is applied by `transform_point`
/// as a ROW vector (`clip = v_row · mvp`), reproducing the hardware's
/// `v · M · V · P` with a sane `w` (`≈ -z_eye`, the perspective depth). See
/// `transform_point` for why applying it as a column vector (`mvp · v`)
/// instead is the transpose and produces the sign-flipping ±thousands `w`.
pub(super) fn recompute_mvp(state: &mut DecodeState) {
    // An ordinary matrix-stack operation supersedes both halves of any
    // force-matrix override and rebuilds the concatenated transform from the
    // public modelview/projection stacks.
    state.pending_forced_mvp = None;
    // A missing projection stack entry means identity projection, not "skip
    // the already-loaded modelview". This function is called only after an
    // actual matrix-stack operation, so the raw-coordinate fixture convention
    // still keeps `mvp == None` until the first G_MTX.
    state.mvp = Some(match state.proj {
        Some(p) => mat_mul(&state.modelview, &p),
        None => state.modelview,
    });
}

/// Load `n` vertices starting at cache slot `v0` from the (segmented) array
/// at `arr_addr`, applying the active transform if one is loaded.
pub(super) fn load_vertices(
    rdram: &[u8],
    state: &mut DecodeState,
    arr_addr: u32,
    n: usize,
    v0: usize,
    family: GeometryWireFamily,
) {
    if matches!(
        family,
        GeometryWireFamily::F3dlx | GeometryWireFamily::F3dlxRej | GeometryWireFamily::F3dlx2Rej
    ) && state.mvp.is_some()
    {
        crate::render_unsupported_panic(
            "render.gbi.geometry.pixel-precision",
            format!(
                "{} transformed G_VTX requires exact pixel-precision rounding that the public manuals do not specify",
                family.name()
            ),
        );
    }
    let base = resolve_addr(&state.segments, arr_addr);
    assert!(
        n > 0
            && v0
                .checked_add(n)
                .is_some_and(|end| end <= state.vtx_cache.len()),
        "G_VTX destination range {v0}..{} is outside cache slots 0..={} or empty",
        v0.saturating_add(n),
        state.vtx_cache.len() - 1
    );
    let byte_len = n
        .checked_mul(VTX_STRIDE)
        .unwrap_or_else(|| panic!("G_VTX count {n} overflows the host address space"));
    let source_end = base.checked_add(byte_len).unwrap_or_else(|| {
        panic!("G_VTX source {base:#x} plus {byte_len} bytes overflows the host address space")
    });
    assert!(
        source_end <= rdram.len(),
        "G_VTX reads past RDRAM: source={base:#x}, count={n}, bytes={byte_len}, rdram_bytes={}",
        rdram.len()
    );
    for i in 0..n {
        let off = base + i * VTX_STRIDE;
        // Swizzled reads (recomp MEM_H / MEM_BU): vertex arrays are DMA'd
        // from ROM through the `^3` per-byte swizzle, same as the DL words.
        let x = read_i16(rdram, off) as f32;
        let y = read_i16(rdram, off + 2) as f32;
        let z = read_i16(rdram, off + 4) as f32;
        // tc[2] (offsets 8, 10): raw S/T in S10.5 fixed-point (§2.1). Scale
        // by the active G_TEXTURE S/T scale, then convert S10.5 -> texels
        // (÷32). The result is texels the rasterizer addresses directly.
        let raw_s = read_i16(rdram, off + 8) as f32;
        let raw_t = read_i16(rdram, off + 10) as f32;
        // cn[4] at offsets 12..16. The alpha byte is always alpha. The RGB
        // bytes are EITHER a flat vertex color (G_LIGHTING off) OR a signed
        // s8 NORMAL (G_LIGHTING on) that must be LIT into a color -- reading
        // a normal as a color is what produced the "rainbow fan" (signed
        // normal components read as unsigned channels). See G_LIGHTING.
        let source_alpha = read_u8(rdram, off + 15);
        let uses_normal = state.geometry_mode & (G_LIGHTING | G_TEXTURE_GEN) != 0;
        let normal = uses_normal.then(|| {
            [
                (read_u8(rdram, off + 12) as i8) as f32 / 127.0,
                (read_u8(rdram, off + 13) as i8) as f32 / 127.0,
                (read_u8(rdram, off + 14) as i8) as f32 / 127.0,
            ]
        });
        let (r, g, b) = if state.geometry_mode & G_LIGHTING != 0 {
            let [lr, lg, lb] = light_vertex(state, normal.expect("lighting normal missing"));
            (lr, lg, lb)
        } else {
            (
                read_u8(rdram, off + 12),
                read_u8(rdram, off + 13),
                read_u8(rdram, off + 14),
            )
        };
        let (s, t) = if state.geometry_mode & G_TEXTURE_GEN != 0 {
            generated_texture_coords(state, normal.expect("texture-generation normal missing"))
        } else {
            (
                raw_s * state.tex.tex_scale_s / 32.0,
                raw_t * state.tex.tex_scale_t / 32.0,
            )
        };

        let (sx, sy, sz, sw, z_screen, clip_code, ndc_z, clip_position) =
            project_vertex(state, x, y, z);
        let a = if state.geometry_mode & G_FOG != 0 {
            fog_alpha(state.fog, ndc_z)
        } else {
            source_alpha
        };
        #[cfg(not(test))]
        {
            projdump::note_pz(sz);
            // On-screen NDC test: perspective-divide the clip coords and check
            // the NDC cube [-1,1]^3 (with a positive-w gate: w<=0 is behind cam).
            let onscreen = if sw > 1e-4 {
                let nx = sx; // sx/sy are already viewport-mapped pixels below;
                let _ = nx;
                // Reconstruct NDC from clip via mvp to classify honestly:
                false
            } else {
                false
            };
            let _ = onscreen;
            if let Some(mvp) = state.mvp {
                let clip = transform_point(&mvp, x, y, z);
                let (cx, cy, cz, cw) = (clip[0], clip[1], clip[2], clip[3]);
                let inside = cw.abs() > 1e-4
                    && (clip[0] / cw).abs() <= 1.0
                    && (clip[1] / cw).abs() <= 1.0
                    && (clip[2] / cw).abs() <= 1.0
                    && cw > 0.0;
                projdump::note_w(cw, inside);
                if projdump::should_log_vtx() {
                    eprintln!(
                        "[FN64_DUMP_PROJ] vtx ob=({x:.0},{y:.0},{z:.0}) -> clip=({cx:.2},{cy:.2},{cz:.2},w={cw:.4}) ndc=({:.3},{:.3},{:.3}) inside_cube={inside}",
                        cx / cw,
                        cy / cw,
                        cz / cw
                    );
                }
            }
        }
        state.vtx_cache[v0 + i] = Vertex {
            x: sx,
            y: sy,
            z: sz,
            r,
            g,
            b,
            a,
            s,
            t,
            w: sw,
            z_screen,
            clip_code,
            clip_position,
        };
        state.vtx_loaded[v0 + i] = true;
    }
}

/// Map a model-space vertex to screen space. If a full projection*modelview
/// is active, apply it, perspective-divide, and map NDC [-1,1] through the
/// explicitly loaded viewport. A matrix without `G_MV_VIEWPORT` is invalid
/// input to this reference path and traps instead of inventing screen state.
/// If NO transform is loaded at all, the raw `ob` x/y are already screen
/// coordinates (the pre-existing reference-fixture convention) and pass
/// through unchanged.
pub(super) fn project_vertex(
    state: &DecodeState,
    x: f32,
    y: f32,
    z: f32,
) -> (f32, f32, f32, f32, u32, u8, f32, Option<[f32; 4]>) {
    if state.persp_normalize.rejects_geometry() {
        // Public `.16` scale zero collapses both transformed coordinates and
        // W before the limited-precision divide. Retain nonpositive W so every
        // primitive path rejects the degenerate result instead of inventing a
        // finite host-float quotient.
        return (0.0, 0.0, 0.0, 0.0, 0, 0, 0.0, Some([0.0; 4]));
    }
    match state.mvp {
        Some(mvp) => {
            let clip = transform_point(&mvp, x, y, z);
            let clip_code = homogeneous_clip_code(clip);
            // Keep the true clip-space w for near-plane culling (a vertex with
            // w <= 0 is at/behind the camera). Guard only the DIVIDE against a
            // near-zero w so the perspective divide doesn't overflow; the
            // decision to draw is made from the un-guarded `clip[3]` (returned
            // as the 4th component) in resolve_tri.
            let true_w = clip[3];
            let w = if true_w.abs() > 1e-6 { true_w } else { 1e-6 };
            let ndc_x = clip[0] / w;
            let ndc_y = clip[1] / w;
            let ndc_z = clip[2] / w;
            let vp = state.viewport.as_ref().expect(
                "G_VTX with an active matrix requires G_MOVEMEM G_MV_VIEWPORT before transformed vertices",
            );
            // vscale/vtrans are in pixels (already /4 in read_viewport).
            let px = ndc_x * vp.sx + vp.tx;
            // N64 screen Y is top-down; NDC +Y is up, so flip.
            let py = -ndc_y * vp.sy + vp.ty;
            let pz = ndc_z * vp.sz + vp.tz;
            (
                px,
                py,
                pz,
                true_w,
                screen_depth_to_fixed(pz),
                clip_code,
                ndc_z,
                Some(clip),
            )
        }
        None => {
            // No transform: raw screen coords (reference-fixture path). w=1 so
            // the near-plane cull never rejects the raw/fixture geometry.
            (x, y, 0.0, 1.0, 0, 0, 0.0, None)
        }
    }
}

pub(super) fn fog_alpha(fog: FogFactor, ndc_z: f32) -> u8 {
    (ndc_z * f32::from(fog.multiplier) + f32::from(fog.offset)).clamp(0.0, 255.0) as u8
}

/// Derive the six clipping-code bits retained by the F3DEX2 vertex cache.
/// Public `gSPCullDisplayList` documentation specifies that volume culling
/// intersects these per-vertex codes and is independent of `gSPClipRatio`.
pub(super) fn homogeneous_clip_code([x, y, z, w]: [f32; 4]) -> u8 {
    let mut code = 0;
    if x < -w {
        code |= CLIP_NEG_X;
    }
    if x > w {
        code |= CLIP_POS_X;
    }
    if y < -w {
        code |= CLIP_NEG_Y;
    }
    if y > w {
        code |= CLIP_POS_Y;
    }
    if z < -w {
        code |= CLIP_NEG_Z;
    }
    if z > w {
        code |= CLIP_POS_Z;
    }
    code
}

pub(super) fn screen_depth_to_fixed(z: f32) -> u32 {
    if !z.is_finite() || z <= 0.0 {
        0
    } else if z >= u32::MAX as f32 / 65536.0 {
        u32::MAX
    } else {
        (z * 65536.0) as u32
    }
}

/// Extract the three F3DEX2 triangle vertex-cache slot indices from a
/// command word: three 7-bit fields at bit offsets 17, 9, 1 (F3DEX2-
/// CONCEPTS.md §2.2). Each field is already the slot (0-31).
pub(super) fn tri_indices(w: u32) -> [u32; 3] {
    [(w >> 17) & 0x7F, (w >> 9) & 0x7F, (w >> 1) & 0x7F]
}

/// A vertex is at/behind the near plane when its clip-space `w` is not
/// positive. Projecting such a vertex divides by a non-positive number and
/// flings it across the screen; a triangle touching one is dropped.
#[inline]
pub(super) fn behind_near_plane(v: &Vertex) -> bool {
    v.w <= 1e-4
}

pub(super) fn resolve_tri(
    vtx_cache: &[Vertex],
    idx: [u32; 3],
    cull: CullMode,
    texture: Option<Texture>,
    other_mode: OtherMode,
    combiner: CombinerState,
    blender: BlenderState,
) -> Option<Triangle> {
    resolve_tri_with_admission(
        vtx_cache,
        idx,
        vtx_cache.len(),
        TriangleAdmission::ClipNear,
        cull,
        texture,
        other_mode,
        combiner,
        blender,
    )
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum TriangleAdmission {
    ClipNear,
    Unclipped,
    RejectBox(ClipRatio),
}

pub(super) fn profile_triangle_admission(
    profile: GeometryUcodeProfile,
    geometry_mode: u32,
    clip_ratio: ClipRatio,
) -> TriangleAdmission {
    let family = profile.wire_family();
    match family {
        GeometryWireFamily::F3dlx if geometry_mode & LEGACY_G_CLIPPING == 0 => {
            TriangleAdmission::Unclipped
        }
        _ if profile.no_n() => TriangleAdmission::Unclipped,
        family if family.is_reject() => TriangleAdmission::RejectBox(clip_ratio),
        _ => TriangleAdmission::ClipNear,
    }
}

pub(super) fn vertex_inside_reject_box(vertex: &Vertex, ratio: ClipRatio) -> bool {
    let Some([x, y, z, w]) = vertex.clip_position else {
        // The raw-coordinate path exists only for deterministic renderer
        // fixtures and has no RSP transform from which a reject box can be
        // reconstructed. Its coordinates are already screen-space input.
        return true;
    };
    x >= -f32::from(ratio.neg_x) * w
        && x <= f32::from(ratio.pos_x) * w
        && y >= -f32::from(ratio.neg_y) * w
        && y <= f32::from(ratio.pos_y) * w
        // The public F3DLX.Rej contract rejects against the far plane but
        // deliberately has no near-plane reject.
        && z <= w
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(super) fn resolve_tri_for_family(
    vtx_cache: &[Vertex],
    idx: [u32; 3],
    family: GeometryWireFamily,
    geometry_mode: u32,
    clip_ratio: ClipRatio,
    cull: CullMode,
    texture: Option<Texture>,
    other_mode: OtherMode,
    combiner: CombinerState,
    blender: BlenderState,
) -> Option<Triangle> {
    resolve_tri_for_profile(
        vtx_cache,
        idx,
        GeometryUcodeProfile::from_public_family(family),
        geometry_mode,
        clip_ratio,
        cull,
        texture,
        other_mode,
        combiner,
        blender,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_tri_for_profile(
    vtx_cache: &[Vertex],
    idx: [u32; 3],
    profile: GeometryUcodeProfile,
    geometry_mode: u32,
    clip_ratio: ClipRatio,
    cull: CullMode,
    texture: Option<Texture>,
    other_mode: OtherMode,
    combiner: CombinerState,
    blender: BlenderState,
) -> Option<Triangle> {
    let family = profile.wire_family();
    resolve_tri_with_admission(
        vtx_cache,
        idx,
        family.cache_capacity(),
        profile_triangle_admission(profile, geometry_mode, clip_ratio),
        cull,
        texture,
        other_mode,
        combiner,
        blender,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_tri_with_admission(
    vtx_cache: &[Vertex],
    idx: [u32; 3],
    cache_capacity: usize,
    admission: TriangleAdmission,
    cull: CullMode,
    texture: Option<Texture>,
    other_mode: OtherMode,
    combiner: CombinerState,
    blender: BlenderState,
) -> Option<Triangle> {
    assert!(
        idx.iter().all(|&i| (i as usize) < cache_capacity),
        "G_TRI vertex-cache slots {idx:?} must all be within 0..={}",
        cache_capacity - 1
    );
    let v = [
        vtx_cache[idx[0] as usize],
        vtx_cache[idx[1] as usize],
        vtx_cache[idx[2] as usize],
    ];
    match admission {
        TriangleAdmission::ClipNear if v.iter().any(behind_near_plane) => return None,
        TriangleAdmission::RejectBox(ratio)
            if v.iter()
                .any(|vertex| !vertex_inside_reject_box(vertex, ratio)) =>
        {
            return None;
        }
        TriangleAdmission::ClipNear
        | TriangleAdmission::Unclipped
        | TriangleAdmission::RejectBox(_) => {}
    }
    Some(Triangle {
        v: [
            vtx_cache[idx[0] as usize],
            vtx_cache[idx[1] as usize],
            vtx_cache[idx[2] as usize],
        ],
        scissor: None,
        cull,
        texture,
        other_mode,
        combiner,
        blender,
    })
}

pub(super) fn resolve_line(
    vtx_cache: &[Vertex],
    slots: [usize; 2],
    width_parameter: u8,
    snapshot: LineDecodeSnapshot,
) -> Option<Line> {
    let [start, end] = slots;
    let Some((&start_vertex, &end_vertex)) = vtx_cache.get(start).zip(vtx_cache.get(end)) else {
        panic!("G_LINE3D cache slots {start} and {end} must both be within F3DEX2 slots 0..=31");
    };
    let [start_vertex, end_vertex] = clip_line_to_homogeneous_volume(
        start_vertex,
        end_vertex,
        snapshot.viewport,
        snapshot.clip_ratio,
    )?;
    Some(Line {
        v: [start_vertex, end_vertex],
        width: 1.5 + f32::from(width_parameter) * 0.5,
        smooth_shading: snapshot.smooth_shading,
        scissor: snapshot.scissor,
        texture: snapshot.texture,
        other_mode: snapshot.other_mode,
        combiner: snapshot.combiner,
        blender: snapshot.blender,
    })
}

pub(super) fn interpolate_line_vertex(start: Vertex, end: Vertex, parameter: f32) -> Vertex {
    let interpolate = |a: f32, b: f32| a + (b - a) * parameter;
    let channel = |a: u8, b: u8| {
        interpolate(f32::from(a), f32::from(b))
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Vertex {
        x: interpolate(start.x, end.x),
        y: interpolate(start.y, end.y),
        z: interpolate(start.z, end.z),
        r: channel(start.r, end.r),
        g: channel(start.g, end.g),
        b: channel(start.b, end.b),
        a: channel(start.a, end.a),
        s: interpolate(start.s, end.s),
        t: interpolate(start.t, end.t),
        w: interpolate(start.w, end.w),
        z_screen: 0,
        clip_code: 0,
        clip_position: None,
    }
}

pub(super) fn project_clipped_line_vertex(
    mut vertex: Vertex,
    clip: [f32; 4],
    viewport: Option<Viewport>,
) -> Vertex {
    let reciprocal_w = 1.0 / clip[3];
    let ndc = [
        clip[0] * reciprocal_w,
        clip[1] * reciprocal_w,
        clip[2] * reciprocal_w,
    ];
    let viewport = viewport
        .expect("clipped transformed G_LINE3D requires the G_MV_VIEWPORT state used by G_VTX");
    vertex.x = ndc[0] * viewport.sx + viewport.tx;
    vertex.y = -ndc[1] * viewport.sy + viewport.ty;
    vertex.z = ndc[2] * viewport.sz + viewport.tz;
    vertex.w = clip[3];
    vertex.z_screen = screen_depth_to_fixed(vertex.z);
    vertex.clip_code = homogeneous_clip_code(clip);
    vertex.clip_position = Some(clip);
    vertex
}

pub(super) fn clip_line_to_homogeneous_volume(
    mut start: Vertex,
    mut end: Vertex,
    viewport: Option<Viewport>,
    clip_ratio: ClipRatio,
) -> Option<[Vertex; 2]> {
    let (Some(mut start_clip), Some(mut end_clip)) = (start.clip_position, end.clip_position)
    else {
        if behind_near_plane(&start) || behind_near_plane(&end) {
            crate::render_unsupported_panic(
                "render.gbi.line.modified-position-clipping",
                "G_LINE3D cannot reconstruct homogeneous clipping after G_MODIFYVTX screen-position writes",
            );
        }
        return Some([start, end]);
    };
    let plane_distance = |clip: [f32; 4], plane: usize| match plane {
        0 => f32::from(clip_ratio.neg_x) * clip[3] + clip[0],
        1 => f32::from(clip_ratio.pos_x) * clip[3] - clip[0],
        2 => f32::from(clip_ratio.neg_y) * clip[3] + clip[1],
        3 => f32::from(clip_ratio.pos_y) * clip[3] - clip[1],
        4 => clip[3] + clip[2],
        5 => clip[3] - clip[2],
        _ => unreachable!(),
    };
    for plane in 0..6 {
        let start_distance = plane_distance(start_clip, plane);
        let end_distance = plane_distance(end_clip, plane);
        if start_distance < 0.0 && end_distance < 0.0 {
            return None;
        }
        if (start_distance < 0.0) != (end_distance < 0.0) {
            let parameter = start_distance / (start_distance - end_distance);
            let clip = std::array::from_fn(|component| {
                start_clip[component] + (end_clip[component] - start_clip[component]) * parameter
            });
            let vertex = interpolate_line_vertex(start, end, parameter);
            if start_distance < 0.0 {
                start = vertex;
                start_clip = clip;
            } else {
                end = vertex;
                end_clip = clip;
            }
        }
    }
    if start_clip[3] <= 1e-6 || end_clip[3] <= 1e-6 {
        return None;
    }
    Some([
        project_clipped_line_vertex(start, start_clip, viewport),
        project_clipped_line_vertex(end, end_clip, viewport),
    ])
}

/// Compatibility declaration for fixture/simple and raw-RDP modes. Geometry
/// mode reports the exact families represented by its digest catalog instead.
pub const SUPPORTED: &[UcodeId] = &[UcodeId::F3dex2];
