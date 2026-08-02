use fn64_render::{
    GeometryUcodeProfile, MicrocodeDataImageIdentity, RenderError, TaskAdmissionGeneration,
    TaskAdmissionSource, UcodeId,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt::Write as _};
use super::*;
use super::wire::*;
use super::types::*;
use super::tmem::*;
use super::state::*;
use super::entries::*;
use super::stream::*;
use super::geometry::*;

/// A 4x4 column-vector transform (row-major storage: `m[row][col]`), f32.
/// Built from an N64 fixed-point `Mtx` (see `read_mtx`) or the identity.
pub(super) type Mat4 = [[f32; 4]; 4];

pub(super) fn identity() -> Mat4 {
    let mut m = [[0.0f32; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}


pub(super) fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for (r, out_row) in out.iter_mut().enumerate() {
        for (c, out_cell) in out_row.iter_mut().enumerate() {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[r][k] * b[k][c];
            }
            *out_cell = s;
        }
    }
    out
}

/// Transform a homogeneous point (x,y,z,1) by `m` using the N64's ROW-VECTOR
/// convention: `clip = v_row · m`, i.e. `out[c] = sum_r v[r] * m[r][c]`.
///
/// The N64 RSP treats vertices as row vectors and matrices in hardware
/// `[row][col]` layout (`clip = v · M · V · P`); `read_mtx` stores each `Mtx`
/// element at `m[row][col]` with NO transpose, and `recompute_mvp` composes
/// `mvp = M · (V · P)` in that same layout. The homogeneous point must
/// therefore be applied on the LEFT as a row vector. Applying it on the RIGHT
/// as a column vector (`m · v`, the old code) computes `mvp^T · v` -- the
/// TRANSPOSE of the true transform. For the perspective MVP that put the
/// projective term (`m[2][3] = -1`) into the OUTPUT ROW instead of the w
/// column, so `w` became `m[3][0]·x + m[3][1]·y + m[3][2]·z` (a huge,
/// sign-flipping value ~±thousands for ob coords of only ±10) instead of the
/// depth `-z_eye`. That is the "giant triangles fanning from a point" bug --
/// vertices with |w|≈thousands and random sign perspective-divide to garbage.
/// Verified against a live OoT gameplay task's decoded P (persp row
/// `[0,0,-1.0016,-1]`) + modelview translation `[-53,-5,0,1]`: column-vector
/// gave `w=-1531.75`; row-vector gives `w=5.0` (= `-z_eye`).
///
/// For a symmetric/diagonal matrix (all the reference-fixture cases exercise)
/// `m == m^T`, so this is identical to the old column-vector product -- the
/// fixture goldens are unchanged. Only the real perspective·view·model
/// product (asymmetric) is affected, which is exactly the gameplay path.
pub(super) fn transform_point(m: &Mat4, x: f32, y: f32, z: f32) -> [f32; 4] {
    let v = [x, y, z, 1.0];
    let mut out = [0.0f32; 4];
    for c in 0..4 {
        let mut s = 0.0;
        for r in 0..4 {
            s += v[r] * m[r][c];
        }
        out[c] = s;
    }
    out
}

/// Read an N64 fixed-point `Mtx` (64 bytes) at `addr` out of `rdram` and
/// convert to an f32 `Mat4`. The N64 `Mtx` layout (gbi.h `Mtx` union,
/// documented public format): the first 32 bytes hold each element's signed
/// integer part as a big-endian s16; the next 32 bytes hold each element's
/// fractional part as a big-endian u16. The real value is
/// `int_part + frac_part / 65536`. Elements are stored row-major
/// (`m[4][4]`). Returns `None` if the 64-byte read would run off `rdram`.
///
/// We store the element (r,c) at `m[r][c]` -- the SAME `[row][col]` layout
/// the hardware `Mtx` (and RT64's `FixedMatrix::toMatrix4x4`) uses, with NO
/// transpose. The N64's row-vector convention (`clip = v_row * M`) is then
/// reproduced by composing the model/view/projection product in hardware
/// order (`recompute_mvp`) and applying it to the vertex as a ROW vector in
/// `transform_point` (`clip = v_row · mvp`). Applying the composed matrix as
/// a COLUMN vector instead (`mvp · v`) computes `mvp^T · v` -- the TRANSPOSE
/// of the true transform -- which put the perspective term into the output
/// row instead of the w column and made `w` a huge sign-flipping value; see
/// `transform_point`'s doc for the cited P/M numbers.
pub(super) fn read_mtx(rdram: &[u8], addr: usize) -> Option<Mat4> {
    if addr + 64 > rdram.len() {
        return None;
    }
    let mut m = [[0.0f32; 4]; 4];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            let elem = r * 4 + c;
            let int_off = addr + elem * 2;
            let frac_off = addr + 32 + elem * 2;
            // Swizzled halfword reads (recomp MEM_H): the Mtx was DMA'd from
            // ROM through the same `^3` per-byte swizzle as everything else.
            let int_part = read_i16(rdram, int_off) as i32;
            let frac_part = read_u16(rdram, frac_off) as i32;
            let value = (((int_part << 16) | frac_part) as f32) / 65536.0;
            // Natural row-major store (hardware [row][col]): NO transpose.
            *cell = value;
        }
    }
    Some(m)
}

/// Read an N64 `Vp` (viewport) struct (16 bytes) at `addr` out of `rdram`
/// and convert to a pixel-space [`Viewport`]. Layout (F3DEX2-CONCEPTS.md
/// §1.4/§3.5): 8 big-endian s16 -- `vscale[4]` (x, y, z, w) then
/// `vtrans[4]` (x, y, z, w), each in the N64 "quarter-pixel" encoding
/// (÷4 for pixel units). Reads through the recomp `^3`/`MEM_H` swizzle
/// like every other DMA'd struct. Returns `None` if the 16-byte read runs
/// off `rdram`.
pub(super) fn read_viewport(rdram: &[u8], addr: usize) -> Option<Viewport> {
    if addr + 16 > rdram.len() {
        return None;
    }
    let vscale_x = read_i16(rdram, addr) as f32;
    let vscale_y = read_i16(rdram, addr + 2) as f32;
    let vscale_z = read_i16(rdram, addr + 4) as f32;
    // addr+6 = vscale.w (unused for screen mapping)
    let vtrans_x = read_i16(rdram, addr + 8) as f32;
    let vtrans_y = read_i16(rdram, addr + 10) as f32;
    let vtrans_z = read_i16(rdram, addr + 12) as f32;
    // addr+14 = vtrans.w (unused)
    let vp = Viewport {
        sx: vscale_x / 4.0,
        sy: vscale_y / 4.0,
        sz: vscale_z / 4.0,
        tx: vtrans_x / 4.0,
        ty: vtrans_y / 4.0,
        tz: vtrans_z / 4.0,
    };
    #[cfg(not(test))]
    if crate::debug_flag("FN64_DUMP_PROJ") {
        eprintln!(
            "[FN64_DUMP_PROJ] viewport: sz={} tz={} => screen-z range [{}, {}] (near->far)",
            vp.sz,
            vp.tz,
            -vp.sz + vp.tz,
            vp.sz + vp.tz
        );
    }
    Some(vp)
}

// --- Vertex lighting (F3DEX2-CONCEPTS.md §2.4) --------------------------

/// Read a `Light_t` (16 bytes, OoT gbi.h:1311 -- `col[3]` u8, pad, `colc[3]`
/// u8, pad, `dir[3]` s8, pad) out of `rdram` at `addr` and install it into
/// light `slot`. Directional slots keep both direction (unit, s8÷127) and
/// color; the ambient slot (`slot == num_dir`) has no meaningful direction,
/// so we ALSO copy its color into `ambient` -- the RSP treats the highest
/// light as pure ambient regardless of its `dir` bytes (gbi.h:2902). Reads
/// through the recomp `^3`/`MEM_B` swizzle like every other DMA'd struct.
pub(super) fn load_light(rdram: &[u8], state: &mut DecodeState, addr: usize, slot: usize) {
    assert!(
        slot < MAX_LIGHTS,
        "G_MOVEMEM G_MV_LIGHT destination slot {slot} exceeds slots 0..{}",
        MAX_LIGHTS - 1
    );
    let end = addr.checked_add(LIGHT_STRIDE).unwrap_or_else(|| {
        panic!("G_MOVEMEM G_MV_LIGHT source {addr:#x} overflows the host address space")
    });
    assert!(
        end <= rdram.len(),
        "G_MOVEMEM G_MV_LIGHT reads past RDRAM: source={addr:#x}, bytes={LIGHT_STRIDE}, rdram_bytes={}",
        rdram.len()
    );
    // col[0..3] at bytes 0..3; dir[3] (s8) at bytes 8..11.
    let cr = read_u8(rdram, addr) as f32 / 255.0;
    let cg = read_u8(rdram, addr + 1) as f32 / 255.0;
    let cb = read_u8(rdram, addr + 2) as f32 / 255.0;
    // dir is signed s8 ÷127 -> a (roughly) unit direction (RSPProcessCS.hlsl
    // `srcNorm / 127`).
    let dx = (read_u8(rdram, addr + 8) as i8) as f32 / 127.0;
    let dy = (read_u8(rdram, addr + 9) as i8) as f32 / 127.0;
    let dz = (read_u8(rdram, addr + 10) as i8) as f32 / 127.0;
    state.lights.dir[slot] = DirLight {
        dir: [dx, dy, dz],
        col: [cr, cg, cb],
    };
    // If this slot is the ambient slot (the one just past the directional
    // count), mirror its color into `ambient`.
    if slot == state.lights.num_dir {
        state.lights.ambient = [cr, cg, cb];
    }
}

/// One destination in the public two-entry `LookAt` structure.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum LookAtAxis {
    X,
    Y,
}

/// F3DEX2 automatic texture-coordinate projection state. Each direction is
/// absent until its corresponding `gSPLookAtX`/`gSPLookAtY` DMA is observed;
/// texture generation cannot manufacture a usable default because the public
/// helpers derive both directions from the active eye/object orientation.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(super) struct LookAtState {
    pub(super) x: Option<[f32; 3]>,
    pub(super) y: Option<[f32; 3]>,
}

/// Decode the direction bytes in one public `Light_t`-shaped LookAt entry.
/// `gdSPDefLookAt` and `guLookAtReflect` place the signed screen-space
/// direction in bytes 8..10; the color bytes are placeholders.
pub(super) fn load_look_at(rdram: &[u8], state: &mut DecodeState, addr: usize, axis: LookAtAxis) {
    let end = addr
        .checked_add(LIGHT_STRIDE)
        .expect("G_MOVEMEM gSPLookAt source range overflows host address space");
    assert!(
        end <= rdram.len(),
        "G_MOVEMEM gSPLookAt {axis:?} reads past RDRAM: source={addr:#x}, bytes={LIGHT_STRIDE}"
    );
    let direction = [
        (read_u8(rdram, addr + 8) as i8) as f32 / 127.0,
        (read_u8(rdram, addr + 9) as i8) as f32 / 127.0,
        (read_u8(rdram, addr + 10) as i8) as f32 / 127.0,
    ];
    match axis {
        LookAtAxis::X => state.look_at.x = Some(direction),
        LookAtAxis::Y => state.look_at.y = Some(direction),
    }
}

/// Decode one of the public F3DEX2 `G_MWO_{a,b}LIGHT_n` destinations.
/// Each light occupies 24 bytes in the microcode state table; the primary and
/// copied colors are the words at offsets 0 and 4 within that stride.
pub(super) fn light_slot_from_moveword_offset(offset: u16) -> Option<usize> {
    let stride_offset = usize::from(offset);
    let word = stride_offset % 24;
    if !matches!(word, 0 | 4) {
        return None;
    }
    let slot = stride_offset / 24;
    (slot < MAX_LIGHTS).then_some(slot)
}

pub(super) fn set_light_color(state: &mut DecodeState, slot: usize, rgba: u32) {
    let [r, g, b, _alpha] = rgba.to_be_bytes();
    let color = [
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    ];
    state.lights.dir[slot].col = color;
    if slot == state.lights.num_dir {
        state.lights.ambient = color;
    }
}

/// Decode the F3DEX2 light slot selected by a `G_MOVEMEM G_MV_LIGHT`
/// destination offset. `gSPLight(..., n)` emits `(n * 24 + 24) / 8` in the
/// wire field, while DMEM indices 0 and 1 are reserved for the two look-at
/// vectors. Therefore `LIGHT_1` starts at DMEM index 2 and maps to light slot
/// 0, matching RT64's `offset / 24 - 2` dispatch.
pub(super) fn light_slot_from_movemem_offset(ofs_div8: usize) -> Option<usize> {
    #[cfg(not(test))]
    let reserved_slots = if std::env::var_os("FN64_DIAG_OLD_LIGHT_SLOT").is_some() {
        1
    } else {
        2
    };
    #[cfg(test)]
    let reserved_slots = 2;
    (ofs_div8 / 3)
        .checked_sub(reserved_slots)
        .filter(|&slot| slot < MAX_LIGHTS)
}

/// Normalize a 3-vector; returns the zero vector unchanged (guards a 0-length
/// normal/direction so a bad DMA can't produce NaN).
#[inline]
pub(super) fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// Rotate a direction (w=0) by the 3x3 upper-left of a `Mat4` (row-major,
/// column-vector convention like `transform_point`). Used to bring a light
/// direction from world/eye space into the vertex's local space, matching
/// RT64's `computeDirLight` (`mul(float4(dir,0), worldMat)`), which multiplies
/// by the modelview so N·L is evaluated in the same space as the (untransformed)
/// vertex normal.
#[inline]
pub(super) fn rotate_dir(m: &Mat4, d: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (r, o) in out.iter_mut().enumerate() {
        *o = m[r][0] * d[0] + m[r][1] * d[1] + m[r][2] * d[2];
    }
    out
}

/// Compute a lit vertex color from a NORMAL (`cn` reinterpreted as s8÷127),
/// the loaded lights, and the current modelview (light-space transform).
/// Ambient + Σ over directionals of `max(N·L, 0) * lightColor`, clamped to
/// [0,1] per channel, returned as u8 RGB. This mirrors RT64's
/// `RSPProcessCS.hlsl` lighting branch (ambient is the base, each directional
/// adds `computeDirLight`, result `min(.,1)`), the microcode-faithful model.
pub(super) fn light_vertex(state: &DecodeState, normal: [f32; 3]) -> [u8; 3] {
    let n = normalize3(normal);
    let mut c = state.lights.ambient;
    for i in 0..state.lights.num_dir {
        let light = &state.lights.dir[i];
        // Bring the light direction into the vertex's (model) space via the
        // modelview, normalize, then N·L (clamped at 0 -- unlit back side
        // contributes nothing).
        let ld = normalize3(rotate_dir(&state.modelview, light.dir));
        let ndotl = (n[0] * ld[0] + n[1] * ld[1] + n[2] * ld[2]).max(0.0);
        c[0] += ndotl * light.col[0];
        c[1] += ndotl * light.col[1];
        c[2] += ndotl * light.col[2];
    }
    [
        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
    ]
}

/// Generate texture coordinates from the public reflection-mapping contract
/// (Programming Manual 11.7.5). Regular mode maps each signed look-at
/// projection from [-1,+1] to [0,scale]. Linear mode maps `acos(projection)`
/// from [0,pi] to [0,scale]. The intermediate 32768 range is S10.5 texture
/// coordinate space before the U0.16 `gSPTexture` scale and `/32` texel
/// conversion already used for explicit vertex coordinates.
pub(super) fn generated_texture_coords(state: &DecodeState, normal: [f32; 3]) -> (f32, f32) {
    assert_ne!(
        state.geometry_mode & G_LIGHTING,
        0,
        "G_TEXTURE_GEN requires G_LIGHTING so vertex cn bytes are normals"
    );
    let look_x = state
        .look_at
        .x
        .expect("G_TEXTURE_GEN requires a preceding gSPLookAtX DMA");
    let look_y = state
        .look_at
        .y
        .expect("G_TEXTURE_GEN requires a preceding gSPLookAtY DMA");

    let n = normalize3(normal);
    let x = normalize3(rotate_dir(&state.modelview, look_x));
    let y = normalize3(rotate_dir(&state.modelview, look_y));
    let project =
        |axis: [f32; 3]| (n[0] * axis[0] + n[1] * axis[1] + n[2] * axis[2]).clamp(-1.0, 1.0);
    let linear = state.geometry_mode & G_TEXTURE_GEN_LINEAR != 0;
    let generated_raw = |projection: f32| {
        if linear {
            projection.acos() / std::f32::consts::PI * 32768.0
        } else {
            (projection + 1.0) * 16384.0
        }
    };
    (
        generated_raw(project(x)) * state.tex.tex_scale_s / 32.0,
        generated_raw(project(y)) * state.tex.tex_scale_t / 32.0,
    )
}
